use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use futures_util::FutureExt;
use margatroid_types::ResourceId;
use mcl_plugin::{load_mcl_program_from_path, MclProgramKind};
use resource_id_plugin::WorldResourceIdExt;
use tokio::io::AsyncReadExt;

use crate::error::{AgentImageLoadError, AgentImageLoadErrorKind, AgentImageTaskError};
use crate::events::{AgentImageReadTask, LoadAgentImageResult};
use crate::types::{
    AgentImageBaseDriver, AgentImageDefaultVisibility, AgentImageDependencies,
    AgentImageDependency, AgentImageLoaderLimits, AgentImageManifest, AgentImageModelConfig,
    AgentImageModelDocument, AgentImageModelParameters, AgentImageReadOutput,
    AgentImageReadPayload, DirectoryEntryKind, DirectoryEntrySignature, DirectorySignature,
    FileSignature, PreparedAgentImage,
};
use crate::{AgentImage, AgentImageLoaderState};

pub(crate) fn handle_agent_image_load(world: &mut World, id: String, reference: ResourceId) {
    if id.is_empty() {
        world.send_event(LoadAgentImageResult {
            id,
            reference,
            result: Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::InvalidRequest,
                "request id cannot be empty",
            )),
        });
        return;
    }

    let task = {
        let state = world
            .get_resource_mut::<AgentImageLoaderState>()
            .expect("AgentImageLoaderPlugin is not installed");
        if let Some(waiting) = state.pending.get_mut(&reference) {
            waiting.push(id);
            None
        } else {
            state.pending.insert(reference.clone(), vec![id]);
            Some(AgentImageReadTask {
                reference,
                root: Arc::clone(&state.root),
                limits: state.limits,
            })
        }
    };
    if let Some(task) = task {
        world.send_async_event(task);
    }
}

pub(crate) async fn read_agent_image(
    task: AgentImageReadTask,
) -> Result<AgentImageReadOutput, AgentImageTaskError> {
    let reference = task.reference.clone();
    let result = std::panic::AssertUnwindSafe(read_agent_image_inner(task))
        .catch_unwind()
        .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::TaskPanicked,
            "agent image loader task panicked",
        )),
    };
    Ok(AgentImageReadOutput::new(AgentImageReadPayload {
        reference,
        result,
    }))
}

async fn read_agent_image_inner(
    task: AgentImageReadTask,
) -> Result<PreparedAgentImage, AgentImageLoadError> {
    let image_root = resolve_image_root(&task.root, &task.reference).await?;
    let layout_before = validate_image_layout(&image_root).await?;

    let manifest_path = image_root.join("agent.toml");
    let (manifest_bytes, manifest_signature) = read_bounded(
        &manifest_path,
        task.limits.max_manifest_bytes,
        AgentImageLoadErrorKind::ManifestReadFailed,
    )
    .await?;
    let manifest_source = std::str::from_utf8(&manifest_bytes).map_err(|_| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::ManifestDecodeFailed,
            "agent.toml is not valid UTF-8",
        )
    })?;
    let manifest: AgentImageManifest = toml::from_str(manifest_source).map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::ManifestDecodeFailed,
            format!("invalid agent.toml: {error}"),
        )
    })?;
    if manifest.schema_version != 1 {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::UnsupportedSchema,
            "agent.toml schema_version must be 1",
        ));
    }
    validate_model_document(&manifest.inference, &task.limits)?;
    let dependencies = manifest
        .dependencies
        .into_iter()
        .map(|dependency| {
            let resource_id = ResourceId::parse(&dependency.id).map_err(|_| {
                AgentImageLoadError::new(
                    AgentImageLoadErrorKind::InvalidResourceName,
                    "agent image dependency ID is invalid",
                )
            })?;
            if dependency.source.as_deref().is_some_and(|source| {
                source.trim().is_empty() || source.chars().any(char::is_control)
            }) {
                return Err(AgentImageLoadError::new(
                    AgentImageLoadErrorKind::InvalidResourceName,
                    "agent image dependency source is empty or contains control characters",
                ));
            }
            Ok(AgentImageDependency {
                resource_id,
                source: dependency.source.map(Arc::from),
            })
        })
        .collect::<Result<Arc<[_]>, _>>()?;
    let base_driver_id = ResourceId::new(
        "mcl",
        task.reference.scope(),
        task.reference.name(),
        Some(task.reference.tag()),
    )
    .map_err(|_| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::BaseMclLoadFailed,
            "AgentImage reference cannot derive a Base Driver resource ID",
        )
    })?;
    let base_driver = load_mcl_program_from_path(
        std::slice::from_ref(&image_root),
        &base_driver_id,
        &image_root.join("base.lua"),
        MclProgramKind::Base,
    )
    .map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::BaseMclLoadFailed,
            format!("Base Lua could not be loaded: {error}"),
        )
    })?;

    validate_prompt_dependencies(&image_root, &dependencies)?;
    let layout_after = validate_image_layout(&image_root).await?;
    if layout_before != layout_after
        || manifest_signature
            != file_signature(&manifest_path, AgentImageLoadErrorKind::ManifestReadFailed).await?
    {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SourceChanged,
            "agent image changed while it was being read",
        ));
    }

    let resources = parse_default_visibility(
        &tokio::fs::read_to_string(image_root.join("base.lua"))
            .await
            .map_err(|_| {
                AgentImageLoadError::new(
                    AgentImageLoadErrorKind::BaseMclLoadFailed,
                    "Base Driver could not be read",
                )
            })?,
        &dependencies,
    )?;
    let inference = manifest.inference;
    Ok(PreparedAgentImage {
        reference: task.reference,
        base_driver: AgentImageBaseDriver {
            program: base_driver,
        },
        dependencies: AgentImageDependencies {
            entries: dependencies,
        },
        model: AgentImageModelConfig {
            model: Arc::from(inference.model),
            parameters: AgentImageModelParameters {
                temperature: inference.temperature,
                max_output_tokens: inference.max_output_tokens,
                top_p: inference.top_p,
                stop: Arc::from(inference.stop),
            },
        },
        default_visibility: AgentImageDefaultVisibility { resources },
    })
}

fn validate_model_document(
    document: &AgentImageModelDocument,
    limits: &AgentImageLoaderLimits,
) -> Result<(), AgentImageLoadError> {
    if document.model.is_empty()
        || document.model.len() > limits.max_model_id_bytes
        || document.model.chars().any(char::is_control)
    {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidModelConfig,
            "model id is empty, too large, or contains a control character",
        ));
    }
    if document.stop.len() > limits.max_stop_sequences
        || document
            .stop
            .iter()
            .any(|sequence| sequence.len() > limits.max_stop_sequence_bytes)
    {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::LimitExceeded,
            "stop sequences exceed the configured loading limit",
        ));
    }
    Ok(())
}

pub(crate) fn apply_agent_image_payload(world: &mut World, payload: AgentImageReadPayload) {
    let waiting = world
        .get_resource_mut::<AgentImageLoaderState>()
        .expect("AgentImageLoaderPlugin is not installed")
        .pending
        .remove(&payload.reference);
    let Some(waiting) = waiting else {
        tracing::error!(reference = %payload.reference, "agent image result has no pending request");
        return;
    };

    match payload.result {
        Ok(prepared) => {
            debug_assert_eq!(prepared.reference, payload.reference);
            let entity = world
                .entity_by_resource_id(&payload.reference)
                .ok()
                .filter(|entity| world.is_alive(*entity))
                .unwrap_or_else(|| world.spawn());
            let PreparedAgentImage {
                base_driver,
                dependencies,
                model,
                default_visibility,
                ..
            } = prepared;
            assert!(world.insert_component(entity, payload.reference.clone()));
            assert!(world.insert_component(
                entity,
                AgentImage {
                    base_driver,
                    dependencies,
                    model,
                    default_visibility,
                },
            ));
            for id in waiting {
                world.send_event(LoadAgentImageResult {
                    id,
                    reference: payload.reference.clone(),
                    result: Ok(entity),
                });
            }
        }
        Err(error) => {
            for id in waiting {
                world.send_event(LoadAgentImageResult {
                    id,
                    reference: payload.reference.clone(),
                    result: Err(error.clone()),
                });
            }
        }
    }
}

async fn resolve_image_root(
    root: &Path,
    reference: &ResourceId,
) -> Result<PathBuf, AgentImageLoadError> {
    if reference.resource_type() != "image" {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidResourceName,
            "agent image resource type must be image",
        ));
    }
    let Some(root) = check_directory(root, true).await? else {
        return Err(AgentImageLoadError::invalid_root(
            "agent image library root does not exist",
        ));
    };
    let mut current = root;
    for part in [reference.scope(), reference.name(), reference.tag()] {
        let candidate = current.join(part);
        let Some(candidate) = check_directory(&candidate, false).await? else {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::NotFound,
                format!("agent image `{reference}` was not found"),
            ));
        };
        current = candidate;
    }
    Ok(current)
}

async fn validate_image_layout(root: &Path) -> Result<DirectorySignature, AgentImageLoadError> {
    let signature = directory_signature(root, 8).await?;
    let mut manifest = false;
    let mut base_driver = false;
    for entry in &signature.entries {
        match (entry.name.to_str(), entry.kind) {
            (Some("agent.toml"), DirectoryEntryKind::File) => manifest = true,
            (Some("base.lua"), DirectoryEntryKind::File) => base_driver = true,
            (Some(name), DirectoryEntryKind::File) if name.ends_with(".md") => {}
            (Some("skills"), DirectoryEntryKind::Directory) => {}
            (Some("hooks"), DirectoryEntryKind::Directory) => {}
            (Some("tools"), DirectoryEntryKind::Directory) => {}
            (Some("shells"), DirectoryEntryKind::Directory) => {}
            _ => {
                return Err(AgentImageLoadError::new(
                    AgentImageLoadErrorKind::InvalidLayout,
                    "agent image contains an unknown entry or invalid entry type",
                ))
            }
        }
    }
    if !manifest || !base_driver {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            "agent image requires agent.toml and base.lua",
        ));
    }
    Ok(signature)
}

fn validate_prompt_dependencies(
    image_root: &Path,
    dependencies: &[AgentImageDependency],
) -> Result<(), AgentImageLoadError> {
    let mut seen = HashSet::new();
    for dependency in dependencies {
        if dependency.resource_id.resource_type() != "prompt" {
            continue;
        }
        let message_key = (
            dependency.resource_id.scope().to_owned(),
            dependency.resource_id.name().to_owned(),
        );
        if !seen.insert(message_key) {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::DuplicateDependency,
                format!(
                    "duplicate prompt message type `prompt:{}:{}` is not allowed",
                    dependency.resource_id.scope(),
                    dependency.resource_id.name()
                ),
            ));
        }
        let role = match dependency.resource_id.scope() {
            "system" | "user" => dependency.resource_id.scope(),
            _ => {
                return Err(AgentImageLoadError::new(
                    AgentImageLoadErrorKind::PromptReadFailed,
                    format!(
                        "prompt dependency `{}` must use scope system or user",
                        dependency.resource_id
                    ),
                ))
            }
        };
        let file_name = format!("{}.md", dependency.resource_id.name().to_uppercase());
        let prompt_path = image_root.join(&file_name);
        if !prompt_path.is_file() {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::PromptReadFailed,
                format!("{file_name} is required by agent.toml prompt dependency but missing"),
            ));
        }
        let _ = role;
    }
    Ok(())
}

pub(crate) fn normalize_root(root: PathBuf) -> Result<PathBuf, AgentImageLoadError> {
    if !root.is_absolute() || has_parent(&root) {
        return Err(AgentImageLoadError::invalid_root(
            "agent image root must be an absolute path without parent traversal",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in root.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => unreachable!("parent components were rejected"),
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

pub(crate) fn ensure_root(root: &Path) -> Result<(), AgentImageLoadError> {
    std::fs::create_dir_all(root).map_err(|error| {
        AgentImageLoadError::invalid_root(format!("cannot create agent image root: {error}"))
    })?;
    let metadata = std::fs::symlink_metadata(root).map_err(|error| {
        AgentImageLoadError::invalid_root(format!("cannot inspect agent image root: {error}"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AgentImageLoadError::invalid_root(
            "agent image root must be a directory and cannot be a symlink",
        ));
    }
    Ok(())
}

async fn check_directory(path: &Path, root: bool) -> Result<Option<PathBuf>, AgentImageLoadError> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AgentImageLoadError::new(
                if root {
                    AgentImageLoadErrorKind::InvalidRoot
                } else {
                    AgentImageLoadErrorKind::InvalidLayout
                },
                format!("cannot inspect agent image directory: {error}"),
            ))
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SymlinkNotAllowed,
            "agent image path cannot contain symlinks",
        ));
    }
    if !metadata.is_dir() {
        return Err(AgentImageLoadError::new(
            if root {
                AgentImageLoadErrorKind::InvalidRoot
            } else {
                AgentImageLoadErrorKind::InvalidLayout
            },
            "agent image path must be a directory",
        ));
    }
    Ok(Some(path.to_path_buf()))
}

async fn directory_signature(
    path: &Path,
    maximum_entries: usize,
) -> Result<DirectorySignature, AgentImageLoadError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            format!("cannot inspect agent image directory: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SymlinkNotAllowed,
            "agent image directory cannot be a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            "agent image path must be a directory",
        ));
    }

    let mut directory = tokio::fs::read_dir(path).await.map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            format!("cannot read agent image directory: {error}"),
        )
    })?;
    let mut entries = Vec::new();
    while let Some(entry) = directory.next_entry().await.map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            format!("cannot read agent image directory entry: {error}"),
        )
    })? {
        let metadata = tokio::fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| {
                AgentImageLoadError::new(
                    AgentImageLoadErrorKind::SourceChanged,
                    format!("agent image directory changed while reading: {error}"),
                )
            })?;
        if metadata.file_type().is_symlink() {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::SymlinkNotAllowed,
                "agent image directory cannot contain symlinks",
            ));
        }
        let kind = if metadata.is_file() {
            DirectoryEntryKind::File
        } else if metadata.is_dir() {
            DirectoryEntryKind::Directory
        } else {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::InvalidLayout,
                "agent image directory cannot contain special files",
            ));
        };
        entries.push(DirectoryEntrySignature {
            name: entry.file_name(),
            kind,
        });
        if entries.len() > maximum_entries {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::LimitExceeded,
                "agent image directory entry count exceeds the configured limit",
            ));
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(DirectorySignature { entries })
}

async fn read_bounded(
    path: &Path,
    maximum: u64,
    read_error: AgentImageLoadErrorKind,
) -> Result<(Vec<u8>, FileSignature), AgentImageLoadError> {
    let before = file_signature(path, read_error).await?;
    if before.length > maximum {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::LimitExceeded,
            "agent image file exceeds the configured limit",
        ));
    }
    let file = tokio::fs::File::open(path).await.map_err(|error| {
        AgentImageLoadError::new(read_error, format!("cannot open agent image file: {error}"))
    })?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| {
            AgentImageLoadError::new(read_error, format!("cannot read agent image file: {error}"))
        })?;
    if bytes.len() as u64 > maximum {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::LimitExceeded,
            "agent image file exceeds the configured limit",
        ));
    }
    let after = file_signature(path, read_error).await?;
    if before != after || bytes.len() as u64 != before.length {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SourceChanged,
            "agent image file changed while it was being read",
        ));
    }
    Ok((bytes, before))
}

async fn file_signature(
    path: &Path,
    read_error: AgentImageLoadErrorKind,
) -> Result<FileSignature, AgentImageLoadError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        AgentImageLoadError::new(
            read_error,
            format!("cannot inspect agent image file: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SymlinkNotAllowed,
            "agent image file cannot be a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            "agent image file must be a regular file",
        ));
    }
    Ok(FileSignature {
        length: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn has_parent(path: &Path) -> bool {
    path.components()
        .any(|component| component == Component::ParentDir)
}

fn parse_default_visibility(
    source: &str,
    dependencies: &[AgentImageDependency],
) -> Result<BTreeSet<ResourceId>, AgentImageLoadError> {
    let mut aliases = HashMap::new();
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("mcl_command(\"IMPORT ") else {
            continue;
        };
        let Some((resource, alias)) = rest.split_once(" AS ") else {
            continue;
        };
        let resource = resource.trim();
        let alias = alias.trim().trim_end_matches("\")");
        if ResourceId::parse(resource).is_ok() {
            aliases.insert(alias.to_owned(), resource.to_owned());
        }
    }
    let Some(inject) = source
        .lines()
        .map(str::trim)
        .find(|line| line.contains("INJECT") && line.contains("TO tool_default"))
    else {
        return Ok(BTreeSet::new());
    };
    let names = inject
        .split("INJECT")
        .nth(1)
        .and_then(|value| value.split("TO tool_default").next())
        .unwrap_or_default();
    let mut result = BTreeSet::new();
    for alias in names.split(',').map(str::trim) {
        let Some(resource) = aliases.get(alias) else {
            continue;
        };
        let resource = ResourceId::parse(resource).map_err(|_| {
            AgentImageLoadError::new(
                AgentImageLoadErrorKind::InvalidResourceName,
                "Base Driver visibility references an invalid resource",
            )
        })?;
        if dependencies
            .iter()
            .any(|dependency| dependency.resource_id == resource)
        {
            result.insert(resource);
        }
    }
    Ok(result)
}
