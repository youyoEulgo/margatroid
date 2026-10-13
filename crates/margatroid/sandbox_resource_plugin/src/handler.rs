use crate::files::{checked_path, error, read_text};
use crate::types::SandboxResourceLoadConfig;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use margatroid_types::agent_components::ImportedValue;
use margatroid_types::resource_components::SandboxResource;
use margatroid_types::resource_events::{mount_resource_import, validate_resource_import};
use margatroid_types::resource_events::{
    LoadSandboxResource, LoadSandboxResourceResult, ResourceLoadError,
    ResourceLoadErrorKind as Kind, SandboxResourceReadOutput, SandboxResourceTaskError,
};
use std::panic::{catch_unwind, AssertUnwindSafe};

struct SubmissionGuard {
    request: Option<LoadSandboxResource>,
    events: app_runtime_plugin::RuntimeEventSender,
}
impl Drop for SubmissionGuard {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            let result = Err(error(
                Kind::TaskFailed,
                "read submission dropped before I/O start",
            ));
            if let Some(sender) = request
                .reply
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                let _ = sender.send(result.clone());
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    self.events.send_event(LoadSandboxResourceResult {
                        id: request.id.clone(),
                        target: request.target,
                        resource_id: request.resource_id.clone(),
                        result,
                    })
                }));
            }
            if let Some(activity) = &request.activity {
                let _ = activity.rollback_submission();
            }
        }
    }
}
pub(crate) fn handle_sandbox_resource_task_failed(
    world: &mut World,
    task: SandboxResourceTaskError,
) {
    if let Some(request) = task.request {
        finish(
            world,
            &request,
            Err(error(Kind::TaskFailed, task.message)),
            false,
        );
    }
}

fn finish(
    world: &World,
    request: &LoadSandboxResource,
    result: Result<(), ResourceLoadError>,
    rollback: bool,
) {
    let sender = request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(sender) = sender {
        let _ = sender.send(result.clone());
        world.send_event(LoadSandboxResourceResult {
            id: request.id.clone(),
            target: request.target,
            resource_id: request.resource_id.clone(),
            result,
        });
    }
    if let Some(activity) = &request.activity {
        let _ = if rollback {
            activity.rollback_submission()
        } else {
            activity.mark_completed()
        };
    }
}
pub(crate) fn handle_load_sandbox_resource(world: &mut World, request: LoadSandboxResource) {
    let invalid = if request.cancel.is_cancelled() {
        Some(error(Kind::Cancelled, "load cancelled"))
    } else if request.id.trim().is_empty() || request.alias.trim().is_empty() {
        Some(error(
            Kind::InvalidRequest,
            "id and alias must not be empty",
        ))
    } else if request.resource_id.resource_type() != "sandbox" {
        Some(error(Kind::InvalidResourceID, "wrong resource type"))
    } else if !world.is_alive(request.target) {
        Some(error(Kind::TargetNotAlive, "target is no longer alive"))
    } else {
        validate_resource_import(
            world,
            request.target,
            &request.resource_id,
            &request.alias,
            request.import_kind,
        )
        .err()
    };
    if let Some(error) = invalid {
        finish(world, &request, Err(error), true);
        return;
    }
    if request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_none()
    {
        if let Some(activity) = &request.activity {
            let _ = activity.rollback_submission();
        }
        return;
    }
    let config = world
        .get_resource::<SandboxResourceLoadConfig>()
        .unwrap()
        .clone();
    let schedule = config.schedule.clone();
    let task_request = request.clone();
    let events = world.event_sender();
    let guard = SubmissionGuard {
        request: Some(request.clone()),
        events: events.clone(),
    };
    let submission = catch_unwind(AssertUnwindSafe(|| {
        world.send_async_closure(&schedule, move || async move {
            let mut guard = guard;
            let (tx, rx) = tokio::sync::oneshot::channel();
            let supervisor_request = task_request.clone();
            let reader_request = task_request.clone();
            let worker = tokio::task::spawn_blocking(move || {
                if let Some(activity) = &reader_request.activity {
                    activity
                        .mark_started()
                        .map_err(|e| error(Kind::TaskFailed, e.to_string()))?;
                }
                prepare(&config, &reader_request)
            });
            guard.request = None;
            tokio::spawn(async move {
                let result = match worker.await {
                    Ok(result) => Ok(SandboxResourceReadOutput {
                        request: supervisor_request,
                        result,
                    }),
                    Err(e) => Err(SandboxResourceTaskError {
                        request: Some(supervisor_request),
                        message: e.to_string(),
                    }),
                };
                if let Err(result) = tx.send(result) {
                    let result: Result<SandboxResourceReadOutput, SandboxResourceTaskError> =
                        match result {
                            Ok(output) => Err(SandboxResourceTaskError {
                                request: Some(output.request),
                                message: "read dispatcher exited after I/O cleanup".into(),
                            }),
                            Err(error) => Err(error),
                        };
                    events.send_event(result);
                }
            });
            rx.await.unwrap_or_else(|_| {
                Err(SandboxResourceTaskError {
                    request: None,
                    message: "read supervisor exited".into(),
                })
            })
        });
    }));
    if submission.is_err() {
        finish(
            world,
            &request,
            Err(error(Kind::TaskFailed, "async submission failed")),
            true,
        );
    }
}
pub(crate) fn handle_mount_sandbox_resource(world: &mut World, output: SandboxResourceReadOutput) {
    let request = output.request;
    let mut reply = request.reply.lock().unwrap_or_else(|e| e.into_inner());
    if reply.is_none() {
        drop(reply);
        if let Some(activity) = &request.activity {
            let _ = activity.mark_completed();
        }
        return;
    }
    let result = if request.cancel.is_cancelled() {
        Err(error(Kind::Cancelled, "load cancelled before mount"))
    } else if !world.is_alive(request.target) {
        Err(error(Kind::TargetNotAlive, "target is no longer alive"))
    } else {
        output.result.and_then(|mut resource| {
            resource.alias = request.alias.clone();
            let imported = if request.import_kind.is_some() {
                Some(ImportedValue::Sandbox(
                    margatroid_types::resource_components::SandboxValue {
                        resource_id: request.resource_id.clone(),
                        policy: resource.policy.clone(),
                    },
                ))
            } else {
                None
            };
            mount_resource_import(
                world,
                request.target,
                &request.resource_id,
                &request.alias,
                request.import_kind,
                resource,
                imported,
            )
        })
    };
    let sender = reply.take().unwrap();
    drop(reply);
    let _ = sender.send(result.clone());
    world.send_event(LoadSandboxResourceResult {
        id: request.id.clone(),
        target: request.target,
        resource_id: request.resource_id.clone(),
        result,
    });
    if let Some(activity) = &request.activity {
        let _ = activity.mark_completed();
    }
}
fn prepare(
    config: &SandboxResourceLoadConfig,
    request: &LoadSandboxResource,
) -> Result<SandboxResource, ResourceLoadError> {
    if request.cancel.is_cancelled() {
        return Err(error(Kind::Cancelled, "load cancelled"));
    }
    let directory = config
        .root
        .join("sandboxes")
        .join(request.resource_id.scope())
        .join(request.resource_id.name())
        .join(request.resource_id.tag());
    checked_path(&directory, true)?;
    let read = |name: &str, maximum| {
        read_text(&directory.join(name), maximum, || {
            request.cancel.is_cancelled()
        })
    };
    let policy = read("policy.json", config.limits.max_policy_bytes)?;
    let value: serde_json::Value =
        serde_json::from_str(&policy).map_err(|e| error(Kind::InvalidDefinition, e.to_string()))?;
    if !value.is_object() {
        return Err(error(
            Kind::InvalidDefinition,
            "policy must be a JSON object",
        ));
    }
    Ok(SandboxResource {
        alias: request.alias.clone(),
        policy: policy.into(),
    })
}
