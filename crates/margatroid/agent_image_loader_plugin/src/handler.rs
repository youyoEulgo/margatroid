#[cfg(test)]
#[path = "supervision_tests.rs"]
mod supervision_tests;

use crate::types::{AgentImageLoadConfig, AgentImageLoadLimits, AgentImageManifest};
use app_runtime_plugin::{RuntimeEventSender, WorldEventExt};
use async_runtime_plugin::{AsyncTaskError, WorldAsyncExt};
use core_plugin::World;
use margatroid_types::agent_image_events::{
    AgentImageReadOutput, ImageLoadError, LoadAgentImage, LoadAgentImageResult,
};
use margatroid_types::resource_components::AgentImage;
use resource_id_plugin::{ResourceID, ResourceSet, WorldResourceExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::io::AsyncReadExt;

fn error(message: impl Into<String>) -> ImageLoadError {
    let mut message = message.into();
    if message.len() > 512 {
        let mut end = 509;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str("...");
    }
    ImageLoadError::new(message)
}
fn result(request: &LoadAgentImage, result: Result<(), ImageLoadError>) -> LoadAgentImageResult {
    LoadAgentImageResult {
        id: request.id.clone(),
        target: request.target,
        resource_id: request.resource_id.clone(),
        result,
    }
}
struct CompletionGuard {
    request: LoadAgentImage,
    events: RuntimeEventSender,
    completed: Arc<AtomicBool>,
}
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if !self.completed.swap(true, Ordering::AcqRel) {
            let event = result(
                &self.request,
                Err(error("image task failed before mount completion")),
            );
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.events.send_event(event)
            }));
        }
    }
}
pub(crate) fn handle_load_agent_image(world: &mut World, request: LoadAgentImage) {
    let invalid = if request.id.trim().is_empty() {
        Some(error("request id cannot be empty"))
    } else if request.resource_id.resource_type() != "image" {
        Some(error("resource type must be image"))
    } else if !world.is_alive(request.target) {
        Some(error("target is not alive"))
    } else {
        None
    };
    if let Some(error) = invalid {
        world.send_event(result(&request, Err(error)));
        return;
    }
    let config = world
        .get_resource::<AgentImageLoadConfig>()
        .expect("image plugin is not installed")
        .clone();
    let schedule = config.schedule.clone();
    let completed = Arc::new(AtomicBool::new(false));
    let guard = Arc::new(CompletionGuard {
        request: request.clone(),
        events: world.event_sender(),
        completed: completed.clone(),
    });
    world.send_async_closure(&schedule, move || async move {
        let result = read_image_files(&config.root, &request.resource_id, &config.limits).await;
        Ok::<_, AsyncTaskError>(AgentImageReadOutput {
            request,
            result,
            completed,
            supervision: guard,
        })
    });
}
pub(crate) fn handle_mount_agent_image(world: &mut World, output: AgentImageReadOutput) {
    if output.completed.load(Ordering::Acquire) {
        return;
    }
    let request = output.request;
    let mounted = output.result.and_then(|image| {
        if !world.is_alive(request.target) {
            return Err(error("target is not alive"));
        }
        let mut set = world
            .get_component::<ResourceSet<AgentImage>>(request.target)
            .cloned()
            .unwrap_or_default();
        set.insert(request.resource_id.clone(), image);
        world
            .insert_resources(request.target, set)
            .map_err(|e| error(format!("mount failed: {e}")))
    });
    world.send_event(result(&request, mounted));
    output.completed.store(true, Ordering::Release);
}
pub(crate) fn normalize_root(root: PathBuf) -> Result<PathBuf, ImageLoadError> {
    if !root.is_absolute() || root.components().any(|part| part == Component::ParentDir) {
        return Err(error(
            "image root must be absolute without parent traversal",
        ));
    }
    Ok(root
        .components()
        .filter(|part| *part != Component::CurDir)
        .collect())
}
async fn check_path(path: &Path, directory: bool) -> Result<(), ImageLoadError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|e| error(format!("cannot inspect image path: {e}")))?;
    if metadata.file_type().is_symlink() {
        return Err(error("image path cannot contain symlinks"));
    }
    if if directory {
        !metadata.is_dir()
    } else {
        !metadata.is_file()
    } {
        return Err(error("image path has an invalid file type"));
    }
    Ok(())
}
async fn read_text_bounded(path: &Path, maximum: u64) -> Result<String, ImageLoadError> {
    check_path(path, false).await?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| error(format!("cannot open image file: {e}")))?;
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| error(format!("cannot read image file: {e}")))?;
    if bytes.len() as u64 > maximum {
        return Err(error("image file exceeds loading limit"));
    }
    String::from_utf8(bytes).map_err(|_| error("image file is not valid UTF-8"))
}
async fn read_image_files(
    root: &Path,
    id: &ResourceID,
    limits: &AgentImageLoadLimits,
) -> Result<AgentImage, ImageLoadError> {
    let mut directory = PathBuf::new();
    for part in root.components() {
        directory.push(part.as_os_str());
        check_path(&directory, true).await?;
    }
    for part in [id.scope(), id.name(), id.tag()] {
        directory.push(part);
        check_path(&directory, true).await?;
    }
    let source =
        read_text_bounded(&directory.join("agent.toml"), limits.max_manifest_bytes).await?;
    let manifest: AgentImageManifest =
        toml::from_str(&source).map_err(|e| error(format!("invalid agent.toml: {e}")))?;
    if manifest.schema_version != 1 {
        return Err(error("agent.toml schema_version must be 1"));
    }
    let model = &manifest.inference;
    if model.model.trim().is_empty()
        || model.model.len() > limits.max_model_id_bytes
        || model.model.chars().any(char::is_control)
    {
        return Err(error("invalid image model id"));
    }
    if model
        .temperature
        .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
        || model
            .top_p
            .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        || model.max_output_tokens == Some(0)
    {
        return Err(error("invalid image model parameters"));
    }
    if model.stop.len() > limits.max_stop_sequences
        || model
            .stop
            .iter()
            .any(|s| s.len() > limits.max_stop_sequence_bytes)
    {
        return Err(error("stop sequences exceed loading limit"));
    }
    let dependencies = manifest
        .dependencies
        .into_iter()
        .map(|d| ResourceID::parse(&d.id).map_err(|e| error(format!("invalid dependency id: {e}"))))
        .collect::<Result<Arc<[_]>, _>>()?;
    let origin = directory.join("base.lua");
    let base_driver = read_text_bounded(&origin, limits.max_script_bytes)
        .await?
        .into();
    Ok(AgentImage {
        base_driver,
        origin: Arc::new(origin),
        dependencies,
        model: manifest.inference,
    })
}
