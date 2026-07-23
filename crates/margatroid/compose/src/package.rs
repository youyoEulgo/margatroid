use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use margatroid_protocol::{
    BundledResource, ContentDigest, RESOURCE_PACKAGE_FORMAT_VERSION, ResourceId, ResourceKind,
    ResourceManifestEntry, ResourcePackage, ResourcePackageFile, ResourceReference,
    SKILL_PACKAGE_MEDIA_TYPE, WORKFLOW_PACKAGE_MEDIA_TYPE,
};
use sha2::{Digest, Sha256};

use crate::compiler::ProjectLimits;
use crate::diagnostic::{ComposeCompileError, ComposeDiagnostic, DiagnosticCode};
use crate::document::{ResourceDetailDocument, ResourceDocument};

pub(crate) struct PackageCollector<'a> {
    project_root: &'a Path,
    main_root: Option<&'a Path>,
    limits: &'a ProjectLimits,
    total_bytes: u64,
    entries: Vec<ResourceManifestEntry>,
    resources: HashMap<ContentDigest, BundledResource>,
    resolved: HashMap<(u8, String), (PathBuf, ResourceReference)>,
}

impl<'a> PackageCollector<'a> {
    pub(crate) fn new(
        project_root: &'a Path,
        main_root: Option<&'a Path>,
        limits: &'a ProjectLimits,
    ) -> Self {
        Self {
            project_root,
            main_root,
            limits,
            total_bytes: 0,
            entries: Vec::new(),
            resources: HashMap::new(),
            resolved: HashMap::new(),
        }
    }

    pub(crate) fn resolve(
        &mut self,
        declaration: &ResourceDocument,
        kind: ResourceKind,
    ) -> Result<ResourceReference, ComposeCompileError> {
        match declaration {
            ResourceDocument::Name(name) => self.resolve_name(name, kind),
            ResourceDocument::Detailed(detail) => self.resolve_detail(detail, kind),
        }
    }

    pub(crate) fn finish(mut self) -> (Vec<ResourceManifestEntry>, Vec<BundledResource>) {
        self.entries.sort_by(|left, right| {
            resource_kind_order(left.kind)
                .cmp(&resource_kind_order(right.kind))
                .then_with(|| left.logical_name.cmp(&right.logical_name))
                .then_with(|| left.digest.as_str().cmp(right.digest.as_str()))
        });
        let mut resources: Vec<_> = self.resources.into_values().collect();
        resources.sort_by(|left, right| left.digest.as_str().cmp(right.digest.as_str()));
        (self.entries, resources)
    }

    fn resolve_name(
        &mut self,
        name: &str,
        kind: ResourceKind,
    ) -> Result<ResourceReference, ComposeCompileError> {
        validate_scoped_name(name)?;
        let subdirectory = kind_directory(kind);
        let project_path = self
            .project_root
            .join(".margatroid")
            .join(subdirectory)
            .join(name);
        if project_path.is_dir() {
            return self.bundle_directory(name, &project_path, self.project_root, kind, None);
        }
        if let Some(main_root) = self.main_root {
            let main_path = main_root.join(subdirectory).join(name);
            if main_path.is_dir() {
                return self.bundle_directory(name, &main_path, main_root, kind, None);
            }
        }
        Err(ComposeDiagnostic::new(
            DiagnosticCode::UnknownReference,
            format!("cannot find {kind:?} package `{name}` in project or main directory"),
        )
        .into())
    }

    fn resolve_detail(
        &mut self,
        detail: &ResourceDetailDocument,
        kind: ResourceKind,
    ) -> Result<ResourceReference, ComposeCompileError> {
        let invalid_fields: Vec<_> = detail
            .extensions
            .keys()
            .filter(|key| !key.starts_with("x-"))
            .cloned()
            .collect();
        if let Some(field) = invalid_fields.first() {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::UnknownField,
                format!("unknown resource field `{field}`"),
            )
            .into());
        }

        match (&detail.path, &detail.installed) {
            (Some(path), None) => {
                let relative = validate_relative_path(path)?;
                let absolute = self.project_root.join(&relative);
                let logical_name = match &detail.name {
                    Some(name) => name.clone(),
                    None => infer_scoped_name(&relative)?,
                };
                validate_scoped_name(&logical_name)?;
                let expected = detail
                    .expected_digest
                    .as_deref()
                    .map(ContentDigest::try_from)
                    .transpose()
                    .map_err(|error| {
                        ComposeCompileError::from(ComposeDiagnostic::new(
                            DiagnosticCode::DigestMismatch,
                            error.to_string(),
                        ))
                    })?;
                self.bundle_directory(
                    &logical_name,
                    &absolute,
                    self.project_root,
                    kind,
                    expected.as_ref(),
                )
            }
            (None, Some(id)) => {
                if detail.expected_digest.is_some() {
                    return Err(ComposeDiagnostic::new(
                        DiagnosticCode::InvalidResource,
                        "expected_digest is only valid for local packages",
                    )
                    .into());
                }
                let id = ResourceId::new(id.clone()).map_err(|error| {
                    ComposeCompileError::from(ComposeDiagnostic::new(
                        DiagnosticCode::InvalidIdentifier,
                        error.to_string(),
                    ))
                })?;
                Ok(ResourceReference::Installed { id })
            }
            _ => Err(ComposeDiagnostic::new(
                DiagnosticCode::InvalidResource,
                "resource object must contain exactly one of path or installed",
            )
            .into()),
        }
    }

    fn bundle_directory(
        &mut self,
        logical_name: &str,
        directory: &Path,
        allowed_root: &Path,
        kind: ResourceKind,
        expected: Option<&ContentDigest>,
    ) -> Result<ResourceReference, ComposeCompileError> {
        let root = canonical_directory(allowed_root)?;
        let directory = fs::canonicalize(directory).map_err(|error| {
            ComposeCompileError::from(ComposeDiagnostic::new(
                DiagnosticCode::Io,
                format!("cannot open package `{logical_name}`: {error}"),
            ))
        })?;
        if !directory.starts_with(&root) {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::PathEscapesProject,
                format!("package `{logical_name}` escapes its resource root"),
            )
            .into());
        }
        if !directory.is_dir() {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::InvalidResource,
                format!("package `{logical_name}` must be a directory"),
            )
            .into());
        }
        let cache_key = (resource_kind_order(kind), logical_name.to_owned());
        if let Some((resolved_path, reference)) = self.resolved.get(&cache_key) {
            if resolved_path != &directory {
                return Err(ComposeDiagnostic::new(
                    DiagnosticCode::DuplicateName,
                    format!("resource `{logical_name}` resolves to multiple packages"),
                )
                .into());
            }
            if let (Some(expected), ResourceReference::Bundled { digest }) = (expected, reference)
                && expected != digest
            {
                return Err(ComposeDiagnostic::new(
                    DiagnosticCode::DigestMismatch,
                    format!("package `{logical_name}` does not match expected digest"),
                )
                .into());
            }
            return Ok(reference.clone());
        }
        if self.entries.len() >= self.limits.max_resources {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::TooManyResources,
                format!("resource count exceeds {}", self.limits.max_resources),
            )
            .into());
        }

        let mut files = BTreeMap::new();
        let mut visited_entries = 0;
        collect_files(
            &directory,
            &directory,
            &root,
            &mut files,
            self.limits.max_files_per_resource,
            &mut visited_entries,
        )?;
        if files.is_empty() {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::InvalidResource,
                format!("package `{logical_name}` is empty"),
            )
            .into());
        }

        let mut encoded_files = Vec::with_capacity(files.len());
        let mut source_bytes = 0_u64;
        for (relative, path) in files {
            let metadata = fs::metadata(&path).map_err(io_diagnostic)?;
            if metadata.len() > self.limits.max_resource_bytes {
                return Err(ComposeDiagnostic::new(
                    DiagnosticCode::ResourceTooLarge,
                    format!("file `{relative}` exceeds the per-file resource limit"),
                )
                .into());
            }
            source_bytes = source_bytes.saturating_add(metadata.len());
            if source_bytes > self.limits.max_resource_bytes {
                return Err(ComposeDiagnostic::new(
                    DiagnosticCode::ResourceTooLarge,
                    format!("package `{logical_name}` source files exceed the resource limit"),
                )
                .into());
            }
            let remaining = self.limits.max_resource_bytes - (source_bytes - metadata.len());
            let bytes = read_limited_file(&path, remaining)?;
            if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                return Err(ComposeDiagnostic::new(
                    DiagnosticCode::InvalidResource,
                    format!("file `{relative}` contains a UTF-8 BOM"),
                )
                .into());
            }
            let bytes = normalize_text(bytes).map_err(|_| {
                ComposeCompileError::from(ComposeDiagnostic::new(
                    DiagnosticCode::InvalidResource,
                    format!("file `{relative}` must be UTF-8 text"),
                ))
            })?;
            encoded_files.push(ResourcePackageFile {
                path: relative,
                content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
        }

        let package_bytes = serde_json::to_vec(&ResourcePackage {
            format_version: RESOURCE_PACKAGE_FORMAT_VERSION,
            files: encoded_files,
        })
        .map_err(|error| {
            ComposeCompileError::from(ComposeDiagnostic::new(
                DiagnosticCode::InvalidResource,
                format!("cannot encode package `{logical_name}`: {error}"),
            ))
        })?;
        let size_bytes = package_bytes.len() as u64;
        if size_bytes > self.limits.max_resource_bytes {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::ResourceTooLarge,
                format!("package `{logical_name}` exceeds the resource limit"),
            )
            .into());
        }
        let digest = digest(&package_bytes);
        let is_new_content = !self.resources.contains_key(&digest);
        let additional_bytes = if is_new_content { size_bytes } else { 0 };
        if self.total_bytes.saturating_add(additional_bytes) > self.limits.max_bundle_bytes {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::BundleTooLarge,
                "bundled resource contents exceed the bundle limit",
            )
            .into());
        }

        if expected.is_some_and(|expected| expected != &digest) {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::DigestMismatch,
                format!("package `{logical_name}` does not match expected digest"),
            )
            .into());
        }
        self.total_bytes += additional_bytes;
        self.entries.push(ResourceManifestEntry {
            kind,
            logical_name: logical_name.to_owned(),
            format_version: RESOURCE_PACKAGE_FORMAT_VERSION,
            digest: digest.clone(),
            size_bytes,
            media_type: media_type(kind).to_owned(),
        });
        self.resources
            .entry(digest.clone())
            .or_insert_with(|| BundledResource {
                digest: digest.clone(),
                content_base64: base64::engine::general_purpose::STANDARD.encode(package_bytes),
            });
        let reference = ResourceReference::Bundled { digest };
        self.resolved
            .insert(cache_key, (directory, reference.clone()));
        Ok(reference)
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, ComposeCompileError> {
    fs::canonicalize(path).map_err(|error| {
        ComposeCompileError::from(ComposeDiagnostic::new(
            DiagnosticCode::Io,
            format!("cannot resolve resource root: {error}"),
        ))
    })
}

fn read_limited_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, ComposeCompileError> {
    let file = fs::File::open(path).map_err(io_diagnostic)?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_diagnostic)?;
    if bytes.len() as u64 > max_bytes {
        return Err(ComposeDiagnostic::new(
            DiagnosticCode::ResourceTooLarge,
            "resource file changed while reading and exceeds the resource limit",
        )
        .into());
    }
    Ok(bytes)
}

fn collect_files(
    package_root: &Path,
    directory: &Path,
    allowed_root: &Path,
    output: &mut BTreeMap<String, PathBuf>,
    max_files: usize,
    visited_entries: &mut usize,
) -> Result<(), ComposeCompileError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(io_diagnostic)? {
        *visited_entries = visited_entries.saturating_add(1);
        if *visited_entries > max_files {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::TooManyFiles,
                format!("resource package contains more than {max_files} filesystem entries"),
            )
            .into());
        }
        entries.push(entry.map_err(io_diagnostic)?);
    }
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if entry.file_type().map_err(io_diagnostic)?.is_symlink() {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::InvalidPath,
                "symbolic links are not allowed inside resource packages",
            )
            .into());
        }
        let path = fs::canonicalize(entry.path()).map_err(io_diagnostic)?;
        if !path.starts_with(allowed_root) {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::PathEscapesProject,
                "package contains a path that escapes its resource root",
            )
            .into());
        }
        let metadata = fs::metadata(&path).map_err(io_diagnostic)?;
        if metadata.is_dir() {
            collect_files(
                package_root,
                &path,
                allowed_root,
                output,
                max_files,
                visited_entries,
            )?;
        } else if metadata.is_file() {
            let relative = path.strip_prefix(package_root).map_err(|_| {
                ComposeCompileError::from(ComposeDiagnostic::new(
                    DiagnosticCode::InvalidPath,
                    "cannot derive package-relative path",
                ))
            })?;
            let relative = relative.to_str().ok_or_else(|| {
                ComposeCompileError::from(ComposeDiagnostic::new(
                    DiagnosticCode::InvalidPath,
                    "package paths must be valid UTF-8",
                ))
            })?;
            output.insert(relative.replace('\\', "/"), path);
        }
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<PathBuf, ComposeCompileError> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(ComposeDiagnostic::new(
            DiagnosticCode::InvalidPath,
            format!("resource path `{value}` must be a project-relative path without `..`"),
        )
        .into());
    }
    Ok(path.to_owned())
}

fn infer_scoped_name(path: &Path) -> Result<String, ComposeCompileError> {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    if components.len() < 2 {
        return Err(ComposeDiagnostic::new(
            DiagnosticCode::InvalidResource,
            "local package path needs an explicit scoped name",
        )
        .into());
    }
    Ok(format!(
        "{}/{}",
        components[components.len() - 2],
        components[components.len() - 1]
    ))
}

fn validate_scoped_name(name: &str) -> Result<(), ComposeCompileError> {
    let mut segments = name.split('/');
    let scope = segments.next().unwrap_or_default();
    let package = segments.next().unwrap_or_default();
    let valid = !scope.is_empty()
        && !package.is_empty()
        && segments.next().is_none()
        && [scope, package].iter().all(|segment| {
            segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        });
    if valid {
        Ok(())
    } else {
        Err(ComposeDiagnostic::new(
            DiagnosticCode::InvalidIdentifier,
            format!("resource name `{name}` must use scope/name syntax"),
        )
        .into())
    }
}

fn normalize_text(bytes: Vec<u8>) -> Result<Vec<u8>, std::string::FromUtf8Error> {
    String::from_utf8(bytes).map(|text| text.replace("\r\n", "\n").replace('\r', "\n").into_bytes())
}

fn digest(bytes: &[u8]) -> ContentDigest {
    let hash = Sha256::digest(bytes);
    ContentDigest::try_from(format!("sha256:{hash:x}")).expect("sha256 output is canonical")
}

fn io_diagnostic(error: std::io::Error) -> ComposeCompileError {
    ComposeDiagnostic::new(DiagnosticCode::Io, format!("resource I/O failed: {error}")).into()
}

fn kind_directory(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Skill => "skills",
        ResourceKind::Workflow => "workflows",
        _ => unreachable!("only package resource kinds are resolved"),
    }
}

fn media_type(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Skill => SKILL_PACKAGE_MEDIA_TYPE,
        ResourceKind::Workflow => WORKFLOW_PACKAGE_MEDIA_TYPE,
        _ => unreachable!("only package resource kinds are bundled"),
    }
}

fn resource_kind_order(kind: ResourceKind) -> u8 {
    match kind {
        ResourceKind::Agent => 0,
        ResourceKind::Soul => 1,
        ResourceKind::Skill => 2,
        ResourceKind::Workflow => 3,
        ResourceKind::Provider => 4,
        _ => u8::MAX,
    }
}
