#[cfg(test)]
#[path = "supervision_tests.rs"]
mod supervision_tests;

use crate::types::{AgentDriverLoadConfig, AgentDriverLoadLimits, AgentDriverManifest};
use app_runtime_plugin::{RuntimeEventSender, WorldEventExt};
use async_runtime_plugin::{AsyncTaskError, WorldAsyncExt};
use core_plugin::World;
use margatroid_types::agent_driver_events::{
    AgentDriverReadOutput, DriverLoadError, LoadAgentDriver, LoadAgentDriverResult,
};
use margatroid_types::resource_components::AgentDriver;
use resource_id_plugin::{ResourceID, ResourceSet, WorldResourceExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::io::AsyncReadExt;

fn error(message: impl Into<String>) -> DriverLoadError {
    DriverLoadError::new(message)
}
fn result(request: &LoadAgentDriver, result: Result<(), DriverLoadError>) -> LoadAgentDriverResult {
    LoadAgentDriverResult {
        id: request.id.clone(),
        target: request.target,
        resource_id: request.resource_id.clone(),
        result: result.map_err(|e| {
            error(format!(
                "request {}, resource {}: {e}",
                request.id, request.resource_id
            ))
        }),
    }
}
struct CompletionGuard {
    request: LoadAgentDriver,
    events: RuntimeEventSender,
    completed: Arc<AtomicBool>,
}
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if !self.completed.swap(true, Ordering::AcqRel) {
            let event = result(
                &self.request,
                Err(error("driver task failed before mount completion")),
            );
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.events.send_event(event)
            }));
        }
    }
}
pub(crate) fn handle_load_agent_driver(world: &mut World, request: LoadAgentDriver) {
    let invalid = if request.id.trim().is_empty() {
        Some(error("request id cannot be empty"))
    } else if request.resource_id.resource_type() != "driver" {
        Some(error("resource type must be driver"))
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
        .get_resource::<AgentDriverLoadConfig>()
        .expect("driver plugin is not installed")
        .clone();
    let schedule = config.schedule.clone();
    let completed = Arc::new(AtomicBool::new(false));
    let guard = Arc::new(CompletionGuard {
        request: request.clone(),
        events: world.event_sender(),
        completed: completed.clone(),
    });
    world.send_async_closure(&schedule, move || async move {
        let result = read_driver_files(&config.root, &request.resource_id, &config.limits).await;
        Ok::<_, AsyncTaskError>(AgentDriverReadOutput {
            request,
            result,
            completed,
            supervision: guard,
        })
    });
}
pub(crate) fn handle_mount_agent_driver(world: &mut World, output: AgentDriverReadOutput) {
    if output.completed.load(Ordering::Acquire) {
        return;
    }
    let request = output.request;
    let mounted = output.result.and_then(|driver| {
        if !world.is_alive(request.target) {
            return Err(error("target is not alive"));
        }
        let mut set = world
            .get_component::<ResourceSet<AgentDriver>>(request.target)
            .cloned()
            .unwrap_or_default();
        set.insert(request.resource_id.clone(), driver);
        world
            .insert_resources(request.target, set)
            .map_err(|e| error(format!("mount failed: {e}")))
    });
    world.send_event(result(&request, mounted));
    output.completed.store(true, Ordering::Release);
}
pub(crate) fn normalize_root(root: PathBuf) -> Result<PathBuf, DriverLoadError> {
    if !root.is_absolute() || root.components().any(|part| part == Component::ParentDir) {
        return Err(error(format!(
            "driver root {} must be absolute without parent traversal",
            root.display()
        )));
    }
    Ok(root
        .components()
        .filter(|part| *part != Component::CurDir)
        .collect())
}
async fn check_path(path: &Path, directory: bool) -> Result<(), DriverLoadError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|e| {
        error(format!(
            "cannot inspect driver path {}: {e}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(error(format!(
            "driver path {} cannot contain symlinks",
            path.display()
        )));
    }
    if if directory {
        !metadata.is_dir()
    } else {
        !metadata.is_file()
    } {
        return Err(error(format!(
            "driver path {} has an invalid file type",
            path.display()
        )));
    }
    Ok(())
}
async fn read_text_bounded(path: &Path, maximum: u64) -> Result<String, DriverLoadError> {
    check_path(path, false).await?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| error(format!("cannot open driver file {}: {e}", path.display())))?;
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| error(format!("cannot read driver file {}: {e}", path.display())))?;
    if bytes.len() as u64 > maximum {
        return Err(error(format!(
            "driver file {} exceeds loading limit {maximum}",
            path.display()
        )));
    }
    String::from_utf8(bytes).map_err(|e| {
        error(format!(
            "driver file {} is not valid UTF-8: {e}",
            path.display()
        ))
    })
}
pub(crate) async fn read_driver_files(
    root: &Path,
    id: &ResourceID,
    limits: &AgentDriverLoadLimits,
) -> Result<AgentDriver, DriverLoadError> {
    let attempted = root.join(id.scope()).join(id.name()).join(id.tag());
    let directory_context = |e| {
        error(format!(
            "resource {id}, attempted directory {}: {e}",
            attempted.display()
        ))
    };
    let mut directory = PathBuf::new();
    for part in root.components() {
        directory.push(part.as_os_str());
        check_path(&directory, true)
            .await
            .map_err(directory_context)?;
    }
    for part in [id.scope(), id.name(), id.tag()] {
        directory.push(part);
        check_path(&directory, true)
            .await
            .map_err(directory_context)?;
    }
    let manifest_path = directory.join("driver.toml");
    let manifest_context = |e| {
        error(format!(
            "resource {id}, manifest {}: {e}",
            manifest_path.display()
        ))
    };
    let manifest_result = async {
        let source =
            read_text_bounded(&directory.join("driver.toml"), limits.max_manifest_bytes).await?;
        let manifest: AgentDriverManifest =
            toml::from_str(&source).map_err(|e| error(format!("invalid driver.toml: {e}")))?;
        if manifest.schema_version != 1 {
            return Err(error("driver.toml schema_version must be 1"));
        }
        let model = &manifest.inference;
        if model.model.trim().is_empty()
            || model.model.len() > limits.max_model_id_bytes
            || model.model.chars().any(char::is_control)
        {
            return Err(error("invalid driver model id"));
        }
        if model
            .temperature
            .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
            || model
                .top_p
                .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
            || model.max_output_tokens == Some(0)
        {
            return Err(error("invalid driver model parameters"));
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
            .map(|d| {
                ResourceID::parse(&d).map_err(|e| error(format!("invalid dependency id {d}: {e}")))
            })
            .collect::<Result<Arc<[_]>, _>>()?;
        Ok((manifest.inference, dependencies))
    }
    .await
    .map_err(manifest_context)?;
    let (model, dependencies) = manifest_result;
    let origin = directory.join("base.lua");
    let base_driver = read_text_bounded(&origin, limits.max_script_bytes)
        .await
        .map_err(|e| error(format!("resource {id}: {e}")))?
        .into();
    Ok(AgentDriver {
        base_driver,
        origin: Arc::new(origin),
        dependencies,
        model,
    })
}
