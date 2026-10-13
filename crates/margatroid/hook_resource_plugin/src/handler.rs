use crate::files::{checked_path, definition, error, field, read_text, schema};
use crate::types::HookResourceLoadConfig;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use margatroid_types::agent_components::ImportedValue;
use margatroid_types::resource_components::HookResource;
use margatroid_types::resource_events::{mount_resource_import, validate_resource_import};
use margatroid_types::resource_events::{
    HookResourceReadOutput, HookResourceTaskError, LoadHookResource, LoadHookResourceResult,
    ResourceLoadError, ResourceLoadErrorKind as Kind,
};
use std::panic::{catch_unwind, AssertUnwindSafe};

struct SubmissionGuard {
    request: Option<LoadHookResource>,
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
                    self.events.send_event(LoadHookResourceResult {
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
pub(crate) fn handle_hook_resource_task_failed(world: &mut World, task: HookResourceTaskError) {
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
    request: &LoadHookResource,
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
        world.send_event(LoadHookResourceResult {
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
pub(crate) fn handle_load_hook_resource(world: &mut World, request: LoadHookResource) {
    let invalid = if request.cancel.is_cancelled() {
        Some(error(Kind::Cancelled, "load cancelled"))
    } else if request.id.trim().is_empty() || request.alias.trim().is_empty() {
        Some(error(
            Kind::InvalidRequest,
            "id and alias must not be empty",
        ))
    } else if request.resource_id.resource_type() != "hook" {
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
        .get_resource::<HookResourceLoadConfig>()
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
                    Ok(result) => Ok(HookResourceReadOutput {
                        request: supervisor_request,
                        result,
                    }),
                    Err(e) => Err(HookResourceTaskError {
                        request: Some(supervisor_request),
                        message: e.to_string(),
                    }),
                };
                if let Err(result) = tx.send(result) {
                    let result: Result<HookResourceReadOutput, HookResourceTaskError> = match result
                    {
                        Ok(output) => Err(HookResourceTaskError {
                            request: Some(output.request),
                            message: "read dispatcher exited after I/O cleanup".into(),
                        }),
                        Err(error) => Err(error),
                    };
                    events.send_event(result);
                }
            });
            rx.await.unwrap_or_else(|_| {
                Err(HookResourceTaskError {
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
pub(crate) fn handle_mount_hook_resource(world: &mut World, output: HookResourceReadOutput) {
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
                Some(ImportedValue::Tool(
                    margatroid_types::resource_components::CallableResource {
                        resource_id: request.resource_id.clone(),
                        alias: request.alias.clone(),
                        definition: std::sync::Arc::new(
                            margatroid_types::resource_components::CallableDefinitionSnapshot::Hook(
                                resource.clone(),
                            ),
                        ),
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
    world.send_event(LoadHookResourceResult {
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
    config: &HookResourceLoadConfig,
    request: &LoadHookResource,
) -> Result<HookResource, ResourceLoadError> {
    if request.cancel.is_cancelled() {
        return Err(error(Kind::Cancelled, "load cancelled"));
    }
    let directory = config
        .root
        .join("hooks")
        .join(request.resource_id.scope())
        .join(request.resource_id.name())
        .join(request.resource_id.tag());
    checked_path(&directory, true)?;
    let read = |name: &str, maximum| {
        read_text(&directory.join(name), maximum, || {
            request.cancel.is_cancelled()
        })
    };
    let metadata = definition(&read("hook.toml", config.limits.max_definition_bytes)?)?;
    let name = field(&metadata, "name")?;
    let description = field(&metadata, "description")?;
    let _ = name;
    let script_path = directory.join("main.lua");
    let input_schema = schema(&read(
        "input.schema.json",
        config.limits.max_definition_bytes,
    )?)?;
    Ok(HookResource {
        alias: request.alias.clone(),
        description,
        input_schema,
        script_path,
    })
}
