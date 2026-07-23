use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use margatroid_protocol::{
    AgentId, AgentImageReference, ResourceKind, ResourceManifest, WorkspaceAgentSpec,
    WorkspaceBundle, WorkspaceName, WorkspaceSpec,
};
use serde::Serialize;

use crate::diagnostic::{ComposeCompileError, ComposeDiagnostic, DiagnosticCode};
use crate::document::{ComposeDocument, invalid_extension_keys};
use crate::package::PackageCollector;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectLimits {
    pub(crate) max_resource_bytes: u64,
    pub(crate) max_bundle_bytes: u64,
    pub(crate) max_compose_bytes: u64,
    pub(crate) max_resources: usize,
    pub(crate) max_files_per_resource: usize,
    pub(crate) max_yaml_aliases: usize,
    pub(crate) max_yaml_depth: usize,
    pub(crate) max_yaml_nodes: usize,
}

impl Default for ProjectLimits {
    fn default() -> Self {
        Self {
            max_resource_bytes: 1024 * 1024,
            max_bundle_bytes: 16 * 1024 * 1024,
            max_compose_bytes: 1024 * 1024,
            max_resources: 1024,
            max_files_per_resource: 4096,
            max_yaml_aliases: 128,
            max_yaml_depth: 64,
            max_yaml_nodes: 100_000,
        }
    }
}

impl ProjectLimits {
    pub fn with_max_resource_bytes(mut self, bytes: u64) -> Self {
        self.max_resource_bytes = bytes;
        self
    }

    pub fn with_max_bundle_bytes(mut self, bytes: u64) -> Self {
        self.max_bundle_bytes = bytes;
        self
    }

    pub fn with_max_compose_bytes(mut self, bytes: u64) -> Self {
        self.max_compose_bytes = bytes;
        self
    }

    pub fn with_max_resources(mut self, count: usize) -> Self {
        self.max_resources = count;
        self
    }

    pub fn with_max_files_per_resource(mut self, count: usize) -> Self {
        self.max_files_per_resource = count;
        self
    }

    pub fn with_max_yaml_aliases(mut self, count: usize) -> Self {
        self.max_yaml_aliases = count;
        self
    }

    pub fn with_max_yaml_depth(mut self, depth: usize) -> Self {
        self.max_yaml_depth = depth;
        self
    }

    pub fn with_max_yaml_nodes(mut self, count: usize) -> Self {
        self.max_yaml_nodes = count;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct CompileOptions {
    workspace_name: Option<String>,
    main_directory: Option<PathBuf>,
    limits: ProjectLimits,
}

impl CompileOptions {
    pub fn with_workspace_name(mut self, name: impl Into<String>) -> Self {
        self.workspace_name = Some(name.into());
        self
    }

    pub fn with_main_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.main_directory = Some(path.into());
        self
    }

    pub fn with_limits(mut self, limits: ProjectLimits) -> Self {
        self.limits = limits;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileOutput {
    normalized: NormalizedProject,
    bundle: WorkspaceBundle,
    warnings: Vec<ComposeDiagnostic>,
}

impl CompileOutput {
    pub fn normalized(&self) -> &NormalizedProject {
        &self.normalized
    }

    pub fn bundle(&self) -> &WorkspaceBundle {
        &self.bundle
    }

    pub fn warnings(&self) -> &[ComposeDiagnostic] {
        &self.warnings
    }

    pub fn into_bundle(self) -> WorkspaceBundle {
        self.bundle
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedProject {
    schema_version: margatroid_protocol::SchemaVersion,
    spec: WorkspaceSpec,
    manifest: ResourceManifest,
}

impl NormalizedProject {
    pub fn schema_version(&self) -> u32 {
        self.schema_version.value()
    }

    pub fn spec(&self) -> &WorkspaceSpec {
        &self.spec
    }

    pub fn manifest(&self) -> &ResourceManifest {
        &self.manifest
    }

    pub fn to_json(&self) -> Result<String, RenderError> {
        serde_json::to_string_pretty(&self.view()).map_err(|error| RenderError(error.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, RenderError> {
        serde_yaml_ng::to_string(&self.view()).map_err(|error| RenderError(error.to_string()))
    }

    fn view(&self) -> NormalizedView<'_> {
        NormalizedView {
            schema_version: self.schema_version,
            spec: &self.spec,
            manifest: &self.manifest,
        }
    }
}

#[derive(Serialize)]
struct NormalizedView<'a> {
    schema_version: margatroid_protocol::SchemaVersion,
    spec: &'a WorkspaceSpec,
    manifest: &'a ResourceManifest,
}

#[derive(Debug)]
pub struct RenderError(String);

impl std::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for RenderError {}

pub struct Compiler {
    options: CompileOptions,
}

impl Compiler {
    pub fn new(options: CompileOptions) -> Self {
        Self { options }
    }

    pub fn compile(
        &self,
        compose_path: impl AsRef<Path>,
    ) -> Result<CompileOutput, ComposeCompileError> {
        let options = &self.options;
        let compose_path = compose_path.as_ref();
        let source_file = compose_path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("margatroid-workspace.yaml"));
        let result: Result<CompileOutput, ComposeCompileError> = (|| {
            let content = read_compose(compose_path, options.limits.max_compose_bytes)?;
            validate_yaml_limits(&content, &options.limits)?;
            let project_root = compose_path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let project_root = fs::canonicalize(project_root).map_err(|error| {
                ComposeCompileError::one(ComposeDiagnostic::new(
                    DiagnosticCode::Io,
                    format!("cannot resolve project root: {error}"),
                ))
            })?;
            let mut yaml: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(&content).map_err(|error| {
                    let location = error.location();
                    let diagnostic = ComposeDiagnostic::new(
                        DiagnosticCode::InvalidYaml,
                        format!("invalid YAML: {error}"),
                    );
                    ComposeCompileError::one(match location {
                        Some(location) => {
                            diagnostic.at_position(location.line(), location.column())
                        }
                        None => diagnostic,
                    })
                })?;
            yaml.apply_merge().map_err(|error| {
                ComposeCompileError::one(ComposeDiagnostic::new(
                    DiagnosticCode::InvalidYaml,
                    format!("invalid YAML merge key: {error}"),
                ))
            })?;
            let mut yaml_nodes = 0;
            validate_yaml_shape(&yaml, &options.limits, 0, &mut yaml_nodes)?;
            let document: ComposeDocument = serde_yaml_ng::from_value(yaml).map_err(|error| {
                let location = error.location();
                let diagnostic = ComposeDiagnostic::new(
                    DiagnosticCode::InvalidYaml,
                    format!("invalid compose document: {error}"),
                );
                ComposeCompileError::one(match location {
                    Some(location) => diagnostic.at_position(location.line(), location.column()),
                    None => diagnostic,
                })
            })?;
            validate_document(&document, options)?;

            let workspace_name = options
                .workspace_name
                .clone()
                .or_else(|| document.workspace.name.clone())
                .or_else(|| {
                    project_root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                })
                .ok_or_else(|| {
                    ComposeCompileError::one(ComposeDiagnostic::new(
                        DiagnosticCode::InvalidIdentifier,
                        "cannot determine workspace name",
                    ))
                })?;
            let name = WorkspaceName::new(workspace_name).map_err(|error| {
                ComposeCompileError::one(ComposeDiagnostic::new(
                    DiagnosticCode::InvalidIdentifier,
                    error.to_string(),
                ))
            })?;
            let manager = AgentId::new(document.workspace.manager.clone()).map_err(|error| {
                ComposeCompileError::one(ComposeDiagnostic::new(
                    DiagnosticCode::InvalidIdentifier,
                    error.to_string(),
                ))
            })?;

            let main_root = options.main_directory.clone().or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".margatroid"))
            });
            let main_root = main_root.filter(|path| path.is_dir());
            let mut collector =
                PackageCollector::new(&project_root, main_root.as_deref(), &options.limits);
            let mut agents = Vec::with_capacity(document.agents.len());
            for (id, agent) in &document.agents {
                let agent_id = AgentId::new(id.clone()).map_err(|error| {
                    ComposeCompileError::one(
                        ComposeDiagnostic::new(
                            DiagnosticCode::InvalidIdentifier,
                            error.to_string(),
                        )
                        .at_field(format!("agents.{id}")),
                    )
                })?;
                let image = AgentImageReference::new(agent.image.clone()).map_err(|error| {
                    ComposeCompileError::one(
                        ComposeDiagnostic::new(
                            DiagnosticCode::InvalidIdentifier,
                            error.to_string(),
                        )
                        .at_field(format!("agents.{id}.image")),
                    )
                })?;
                let skills = agent
                    .skills
                    .iter()
                    .enumerate()
                    .map(|(index, resource)| {
                        collector
                            .resolve(resource, ResourceKind::Skill)
                            .map_err(|error| {
                                error.with_field(format!("agents.{id}.skills[{index}]"))
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let workflows = agent
                    .workflows
                    .iter()
                    .enumerate()
                    .map(|(index, resource)| {
                        collector
                            .resolve(resource, ResourceKind::Workflow)
                            .map_err(|error| {
                                error.with_field(format!("agents.{id}.workflows[{index}]"))
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                agents.push(WorkspaceAgentSpec {
                    id: agent_id,
                    image,
                    skills,
                    workflows,
                    memory_volume: agent.memory_volume.clone(),
                });
            }
            let (entries, resources) = collector.finish();
            let bundle = WorkspaceBundle {
                schema_version: margatroid_protocol::SchemaVersion::current(),
                spec: WorkspaceSpec {
                    name,
                    description: document.workspace.description,
                    manager,
                    agents,
                },
                manifest: ResourceManifest { entries },
                resources,
            };
            let normalized = NormalizedProject {
                schema_version: bundle.schema_version,
                spec: bundle.spec.clone(),
                manifest: bundle.manifest.clone(),
            };
            Ok(CompileOutput {
                normalized,
                bundle,
                warnings: Vec::new(),
            })
        })();
        result.map_err(|error| error.with_source_file(source_file))
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new(CompileOptions::default())
    }
}

pub fn compile(compose_path: impl AsRef<Path>) -> Result<CompileOutput, ComposeCompileError> {
    Compiler::default().compile(compose_path)
}

fn read_compose(path: &Path, max_bytes: u64) -> Result<String, ComposeCompileError> {
    let file = fs::File::open(path).map_err(|error| {
        ComposeCompileError::one(ComposeDiagnostic::new(
            DiagnosticCode::Io,
            format!("cannot read compose file: {error}"),
        ))
    })?;
    if file
        .metadata()
        .map_err(|error| {
            ComposeCompileError::one(ComposeDiagnostic::new(
                DiagnosticCode::Io,
                format!("cannot inspect compose file: {error}"),
            ))
        })?
        .len()
        > max_bytes
    {
        return Err(ComposeDiagnostic::new(
            DiagnosticCode::ComposeTooLarge,
            format!("compose file exceeds the {max_bytes} byte limit"),
        )
        .into());
    }
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ComposeCompileError::one(ComposeDiagnostic::new(
                DiagnosticCode::Io,
                format!("cannot read compose file: {error}"),
            ))
        })?;
    if bytes.len() as u64 > max_bytes {
        return Err(ComposeDiagnostic::new(
            DiagnosticCode::ComposeTooLarge,
            format!("compose file exceeds the {max_bytes} byte limit"),
        )
        .into());
    }
    String::from_utf8(bytes).map_err(|_| {
        ComposeDiagnostic::new(DiagnosticCode::InvalidYaml, "compose file must be UTF-8").into()
    })
}

fn validate_yaml_limits(content: &str, limits: &ProjectLimits) -> Result<(), ComposeCompileError> {
    let aliases = count_yaml_aliases(content);
    if aliases > limits.max_yaml_aliases {
        return Err(ComposeDiagnostic::new(
            DiagnosticCode::InvalidYaml,
            format!("YAML alias count exceeds {}", limits.max_yaml_aliases),
        )
        .into());
    }
    Ok(())
}

fn count_yaml_aliases(content: &str) -> usize {
    let mut aliases = 0;
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    let mut comment = false;
    let mut previous = '\n';

    for character in content.chars() {
        if comment {
            if character == '\n' {
                comment = false;
            }
            previous = character;
            continue;
        }
        if double_quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                double_quoted = false;
            }
            previous = character;
            continue;
        }
        if single_quoted {
            if character == '\'' {
                single_quoted = false;
            }
            previous = character;
            continue;
        }
        match character {
            '#' => comment = true,
            '"' => double_quoted = true,
            '\'' => single_quoted = true,
            '*' if previous.is_whitespace() || matches!(previous, '[' | '{' | ',' | ':') => {
                aliases += 1;
            }
            _ => {}
        }
        previous = character;
    }
    aliases
}

fn validate_yaml_shape(
    value: &serde_yaml_ng::Value,
    limits: &ProjectLimits,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), ComposeCompileError> {
    *nodes = nodes.saturating_add(1);
    if *nodes > limits.max_yaml_nodes {
        return Err(ComposeDiagnostic::new(
            DiagnosticCode::InvalidYaml,
            format!("YAML node count exceeds {}", limits.max_yaml_nodes),
        )
        .into());
    }
    if depth > limits.max_yaml_depth {
        return Err(ComposeDiagnostic::new(
            DiagnosticCode::InvalidYaml,
            format!("YAML depth exceeds {}", limits.max_yaml_depth),
        )
        .into());
    }
    match value {
        serde_yaml_ng::Value::Sequence(values) => {
            for value in values {
                validate_yaml_shape(value, limits, depth + 1, nodes)?;
            }
        }
        serde_yaml_ng::Value::Mapping(values) => {
            for (key, value) in values {
                validate_yaml_shape(key, limits, depth + 1, nodes)?;
                validate_yaml_shape(value, limits, depth + 1, nodes)?;
            }
        }
        serde_yaml_ng::Value::Tagged(_) => {
            return Err(ComposeDiagnostic::new(
                DiagnosticCode::InvalidYaml,
                "explicit YAML tags are not supported",
            )
            .into());
        }
        _ => {}
    }
    Ok(())
}

fn validate_document(
    document: &ComposeDocument,
    options: &CompileOptions,
) -> Result<(), ComposeCompileError> {
    let mut errors = Vec::new();
    if document.schema_version != 1 {
        errors.push(
            ComposeDiagnostic::new(
                DiagnosticCode::UnsupportedSchema,
                format!(
                    "unsupported compose schema version {}",
                    document.schema_version
                ),
            )
            .at_field("schema_version"),
        );
    }
    for key in invalid_extension_keys([
        ("", &document.extensions),
        ("workspace", &document.workspace.extensions),
    ]) {
        errors.push(
            ComposeDiagnostic::new(
                DiagnosticCode::UnknownField,
                format!("unknown field `{key}`; only x-* extensions are allowed"),
            )
            .at_field(key),
        );
    }
    if document.agents.is_empty() {
        errors.push(
            ComposeDiagnostic::new(
                DiagnosticCode::InvalidResource,
                "agents must contain at least one AgentInstance",
            )
            .at_field("agents"),
        );
    }
    if !document.agents.contains_key(&document.workspace.manager) {
        errors.push(
            ComposeDiagnostic::new(
                DiagnosticCode::MissingManager,
                format!(
                    "manager `{}` is not present in agents",
                    document.workspace.manager
                ),
            )
            .at_field("workspace.manager"),
        );
    }
    let mut volume_names = BTreeSet::new();
    for (name, volume) in &document.volumes {
        if !volume_names.insert(name) {
            errors.push(
                ComposeDiagnostic::new(
                    DiagnosticCode::DuplicateName,
                    format!("duplicate volume `{name}`"),
                )
                .at_field(format!("volumes.{name}")),
            );
        }
        if volume.extensions.keys().any(|key| !key.starts_with("x-")) {
            errors.push(
                ComposeDiagnostic::new(
                    DiagnosticCode::UnknownField,
                    format!("unknown field in volume `{name}`"),
                )
                .at_field(format!("volumes.{name}")),
            );
        }
    }
    for (id, agent) in &document.agents {
        let agent_field = format!("agents.{id}");
        for key in invalid_extension_keys([(agent_field.as_str(), &agent.extensions)]) {
            errors.push(
                ComposeDiagnostic::new(
                    DiagnosticCode::UnknownField,
                    format!("unknown field `{key}`; only x-* extensions are allowed"),
                )
                .at_field(key),
            );
        }
        if let Some(volume) = &agent.memory_volume
            && !document.volumes.contains_key(volume)
        {
            errors.push(
                ComposeDiagnostic::new(
                    DiagnosticCode::UnknownReference,
                    format!("agent `{id}` references unknown volume `{volume}`"),
                )
                .at_field(format!("agents.{id}.memory_volume")),
            );
        }
    }
    if options
        .workspace_name
        .as_deref()
        .is_some_and(|name| name.is_empty())
    {
        errors.push(
            ComposeDiagnostic::new(
                DiagnosticCode::InvalidIdentifier,
                "workspace override cannot be empty",
            )
            .at_field("workspace.name"),
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ComposeCompileError::many(errors))
    }
}
