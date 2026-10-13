use crate::{
    RunnerRequest, ToolContext, ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecuteResponse,
    ToolPlugin,
};
use async_runtime_plugin::{AsyncTaskError, WorldAsyncExt};
use core_plugin::World;
use futures_util::FutureExt;
use margatroid_types::agent_components::{Agent, AgentState};
use margatroid_types::execution_components::{
    ExecutionActivity, ExecutionActivityKind, ExecutionError, SandboxPolicySnapshot,
};
use margatroid_types::message_types::AgentMessage;
use margatroid_types::resource_components::{
    skill_input_schema, CallableDefinitionSnapshot, CallableResource, HookResource, SkillResource,
    ToolResource,
};
use resource_id_plugin::ResourceSet;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

pub(crate) fn reply(request: &ToolExecuteRequest, result: Result<ToolExecuteResponse, ToolError>) {
    if let Some(sender) = request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let _ = sender.send(result);
    }
}
pub(crate) fn receiver_closed(request: &ToolExecuteRequest) -> bool {
    request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_none_or(|s| s.is_closed())
}
struct Prepared {
    request: ToolExecuteRequest,
    path: PathBuf,
    skill: bool,
    arguments: serde_json::Value,
    context: ToolContext,
    activity: ExecutionActivity,
    config: ToolPlugin,
    started: bool,
    finished: bool,
}
impl Drop for Prepared {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.started {
            self.activity
                .mark_cleanup_failed(ExecutionError::CleanupFailed {
                    activity_id: self.activity.id,
                    message: "execution owner dropped before cleanup".into(),
                });
            reply(
                &self.request,
                Err(ToolError::new(
                    ToolErrorKind::CleanupFailed,
                    "execution owner dropped before cleanup",
                )),
            );
        } else {
            let _ = self.activity.rollback_submission();
            reply(
                &self.request,
                Err(ToolError::new(
                    ToolErrorKind::SubmitFailed,
                    "execution not submitted",
                )),
            );
        }
    }
}
fn validate_owner(
    world: &World,
    entity: core_plugin::Entity,
    selected: &CallableResource,
) -> Result<(), ToolError> {
    let id = &selected.resource_id;
    let present = match id.resource_type() {
        "tool" => world
            .get_component::<ResourceSet<ToolResource>>(entity)
            .is_some_and(|set| set.get(id).is_some()),
        "skill" => world
            .get_component::<ResourceSet<SkillResource>>(entity)
            .is_some_and(|set| set.get(id).is_some()),
        "hook" => world
            .get_component::<ResourceSet<HookResource>>(entity)
            .is_some_and(|set| set.get(id).is_some()),
        _ => {
            return Err(ToolError::new(
                ToolErrorKind::UnsupportedResource,
                "resource is not callable",
            ))
        }
    };
    if !present {
        return Err(ToolError::new(
            ToolErrorKind::ResourceMissing,
            "resource not loaded on target agent",
        ));
    }
    let owner = world
        .get_component::<Agent>(entity)
        .ok_or_else(|| ToolError::new(ToolErrorKind::ResourceMissing, "resource owner missing"))?;
    if !owner.info.image_dependencies.contains(id)
        || !owner
            .import_bindings
            .contains_resource(id, margatroid_types::block_types::InnerType::Tool)
    {
        return Err(ToolError::new(
            ToolErrorKind::ResourceMissing,
            "resource is not imported or permitted for target agent",
        ));
    }
    Ok(())
}
fn prepare(world: &World, request: ToolExecuteRequest) -> Result<Prepared, ToolError> {
    if request.id.trim().is_empty() || request.call.id.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "empty request or call ID",
        ));
    }
    if request.cancel.is_cancelled() || receiver_closed(&request) {
        return Err(ToolError::new(
            ToolErrorKind::Cancelled,
            "request cancelled",
        ));
    }
    if !world.is_alive(request.agent) {
        return Err(ToolError::new(
            ToolErrorKind::AgentNotAlive,
            "agent no longer alive",
        ));
    }
    let agent = world
        .get_component::<Agent>(request.agent)
        .ok_or_else(|| ToolError::new(ToolErrorKind::AgentNotAlive, "Agent missing"))?;
    if !matches!(agent.state, AgentState::Running) {
        return Err(ToolError::new(
            ToolErrorKind::AgentNotAlive,
            "agent not running",
        ));
    }
    let config = world
        .get_resource::<ToolPlugin>()
        .ok_or_else(|| ToolError::new(ToolErrorKind::SubmitFailed, "ToolPlugin missing"))?
        .clone();
    if request.call.arguments.len() > config.limits.argument_bytes {
        return Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "arguments exceed limit",
        ));
    }
    let arguments: serde_json::Value = serde_json::from_str(&request.call.arguments)
        .map_err(|_| ToolError::new(ToolErrorKind::InvalidArguments, "arguments must be JSON"))?;
    validate_owner(world, request.agent, &request.resource)?;
    if request.call.tool_name != request.resource.alias {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "call name does not match selected alias",
        ));
    }
    let (path, schema, skill) = match (
        &*request.resource.definition,
        request.resource.resource_id.resource_type(),
    ) {
        (CallableDefinitionSnapshot::Tool(resource), "tool") => (
            resource.script_path.clone(),
            resource.input_schema.clone(),
            false,
        ),
        (CallableDefinitionSnapshot::Skill(resource), "skill") => {
            (resource.content_path.clone(), skill_input_schema(), true)
        }
        (CallableDefinitionSnapshot::Hook(_), "hook") => {
            return Err(ToolError::new(
                ToolErrorKind::UnsupportedHook,
                "hook execution route is not supported",
            ))
        }
        _ => {
            return Err(ToolError::new(
                ToolErrorKind::UnsupportedResource,
                "snapshot kind does not match resource identity",
            ))
        }
    };
    fn has_external_reference(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
                (key == "$ref"
                    && value
                        .as_str()
                        .is_some_and(|reference| !reference.starts_with('#')))
                    || has_external_reference(value)
            }),
            serde_json::Value::Array(values) => values.iter().any(has_external_reference),
            _ => false,
        }
    }
    if has_external_reference(&schema) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "external schema references are not allowed",
        ));
    }
    let validator = jsonschema::validator_for(&schema)
        .map_err(|_| ToolError::new(ToolErrorKind::InvalidDefinition, "invalid tool schema"))?;
    if !validator.is_valid(&arguments) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "arguments do not match schema",
        ));
    }
    let kind = if skill {
        ExecutionActivityKind::Other
    } else {
        ExecutionActivityKind::Sandbox
    };
    let activity = agent
        .execution
        .register_with_snapshot(kind, || {
            agent
                .sandbox
                .as_ref()
                .map(|selected| {
                    Ok(SandboxPolicySnapshot {
                        resource_id: selected.resource_id.clone(),
                        policy: selected.policy.clone(),
                    })
                })
                .transpose()
        })
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::ExecutionBusy,
                "execution scope refused admission",
            )
        })?;
    let context = ToolContext {
        agent_id: agent.resourceid.to_string(),
        resource_id: request.resource.resource_id.to_string(),
        call_id: request.call.id.clone(),
        turn: request.turn.clone(),
        project_root: agent.info.project_root.clone(),
        temp_dir: PathBuf::new(),
    };
    Ok(Prepared {
        request,
        path,
        skill,
        arguments,
        context,
        activity,
        config,
        started: false,
        finished: false,
    })
}
#[derive(Debug)]
struct SupervisionError;
impl From<AsyncTaskError> for SupervisionError {
    fn from(_: AsyncTaskError) -> Self {
        Self
    }
}

pub(crate) fn handle_tool_execute(world: &mut World, mut request: ToolExecuteRequest) {
    let sender = request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let Some(sender) = sender else {
        return;
    };
    request.reply = std::sync::Arc::new(std::sync::Mutex::new(Some(sender)));
    let prepared = match prepare(world, request.clone()) {
        Ok(p) => p,
        Err(e) => {
            reply(&request, Err(e));
            return;
        }
    };
    let schedule = prepared.config.schedule.clone();
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(prepared)));
    let transferred = slot.clone();
    let submitted = std::panic::catch_unwind(AssertUnwindSafe(|| {
        world.send_async_closure(&schedule, move || async move {
            let prepared = transferred.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(prepared) = prepared {
                supervise(prepared).await;
            }
            Ok::<(), SupervisionError>(())
        });
    }));
    if submitted.is_err() {
        if let Some(prepared) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = prepared.activity.rollback_submission();
            reply(
                &prepared.request,
                Err(ToolError::new(
                    ToolErrorKind::SubmitFailed,
                    "async submission failed",
                )),
            );
        }
    }
}
async fn supervise(prepared: Prepared) {
    let cancellation = prepared.request.cancel.clone().drop_guard();
    let supervisor = tokio::spawn(execute(prepared));
    let _ = supervisor.await;
    cancellation.disarm();
}
async fn bounded_file(path: &std::path::Path, limit: usize) -> Result<String, ToolError> {
    let file = tokio::fs::File::open(path).await.map_err(|_| {
        ToolError::new(
            ToolErrorKind::ScriptReadFailed,
            "resource could not be opened",
        )
    })?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| ToolError::new(ToolErrorKind::ScriptReadFailed, "resource read failed"))?;
    if bytes.len() > limit {
        return Err(ToolError::new(
            ToolErrorKind::OutputTooLarge,
            "resource exceeds limit",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| ToolError::new(ToolErrorKind::ScriptReadFailed, "resource not UTF-8"))
}
async fn execute(mut prepared: Prepared) {
    if let Err(_) = prepared.activity.mark_started() {
        reply(
            &prepared.request,
            Err(ToolError::new(ToolErrorKind::Cancelled, "activity missing")),
        );
        return;
    }
    prepared.started = true;
    let started_at = tokio::time::Instant::now();
    let result = AssertUnwindSafe(async {
        let timeout = tokio::time::sleep(prepared.config.limits.execution_timeout);
        tokio::pin!(timeout);
        let file = bounded_file(&prepared.path, if prepared.skill { prepared.config.limits.output_bytes } else { prepared.config.limits.script_bytes });
        let text = tokio::select! {
            result = file => result?,
            _ = prepared.activity.cancel.cancelled() => return Err(ToolError::new(ToolErrorKind::Cancelled, "scope cancelled")),
            _ = prepared.request.cancel.cancelled() => return Err(ToolError::new(ToolErrorKind::Cancelled, "request cancelled")),
            _ = &mut timeout => return Err(ToolError::new(ToolErrorKind::Timeout, "resource read timed out")),
            _ = crate::process::closed(&prepared.request.reply) => return Err(ToolError::new(ToolErrorKind::Cancelled, "receiver closed")),
        };
        if prepared.activity.cancel.is_cancelled() || prepared.request.cancel.is_cancelled() || receiver_closed(&prepared.request) { return Err(ToolError::new(ToolErrorKind::Cancelled, "request cancelled")); }
        if prepared.skill { return Ok(text); }
        let temp = tempfile::Builder::new().prefix("margatroid-tool-").tempdir().map_err(|_| ToolError::new(ToolErrorKind::PreparationFailed, "temporary directory creation failed"))?;
        prepared.context.temp_dir = temp.path().to_path_buf();
        let mut limits = prepared.config.limits.clone();
        limits.execution_timeout = limits.execution_timeout.saturating_sub(started_at.elapsed());
        limits.host_call_timeout = limits.host_call_timeout.min(limits.execution_timeout);
        if limits.execution_timeout.is_zero() { return Err(ToolError::new(ToolErrorKind::Timeout, "preparation exhausted deadline")); }
        let payload = RunnerRequest { protocol_version: 1, script: text, arguments: prepared.arguments.clone(), context: prepared.context.clone(), limits };
        let result = crate::process::run_runner(&prepared.config.runner_path, payload, prepared.activity.sandbox_snapshot.as_ref().map(|s| s.policy.as_ref()), &prepared.activity.cancel, &prepared.request.cancel, &prepared.request.reply).await;
        if result.as_ref().is_err_and(|e| e.kind == ToolErrorKind::CleanupFailed) { let _ = temp.keep(); return result; }
        if temp.close().is_err() { return Err(ToolError::new(ToolErrorKind::CleanupFailed, "temporary directory cleanup failed")); }
        result
    }).catch_unwind().await.unwrap_or_else(|_| Err(ToolError::new(ToolErrorKind::CleanupFailed, "worker panicked; cleanup not proven")));
    if result
        .as_ref()
        .is_err_and(|e| e.kind == ToolErrorKind::CleanupFailed)
    {
        prepared
            .activity
            .mark_cleanup_failed(ExecutionError::CleanupFailed {
                activity_id: prepared.activity.id,
                message: "tool execution cleanup failed".into(),
            });
    } else {
        let _ = prepared.activity.mark_completed();
    }
    let result = result.map(|content| ToolExecuteResponse {
        message: AgentMessage::ToolCall {
            resource_id: Some(prepared.request.resource.resource_id.clone()),
            tool_call_id: prepared.request.call.id.clone(),
            tool_name: prepared.request.call.tool_name.clone(),
            content,
            failed: false,
        },
    });
    reply(&prepared.request, result);
    prepared.finished = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use margatroid_types::execution_components::AgentExecutionScope;
    use margatroid_types::message_types::ToolCall;
    use resource_id_plugin::ResourceID;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn caller_abort_leaves_owned_supervisor_to_finish_receipt_and_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("skill.md");
        std::fs::write(&path, "body").unwrap();
        let scope = AgentExecutionScope::new();
        let activity = scope.register(ExecutionActivityKind::Other).unwrap();
        let (tx, rx) = oneshot::channel();
        let resource = ResourceID::parse("skill:test/skill:latest").unwrap();
        let request = ToolExecuteRequest {
            id: "req".into(),
            agent: World::default().spawn(),
            resource: CallableResource {
                resource_id: resource.clone(),
                alias: "skill".into(),
                definition: Arc::new(CallableDefinitionSnapshot::Skill(SkillResource {
                    alias: "skill".into(),
                    description: "skill".into(),
                    content_path: path.clone(),
                })),
            },
            call: ToolCall {
                id: "call".into(),
                tool_name: "skill".into(),
                arguments: "{}".into(),
            },
            turn: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            reply: Arc::new(Mutex::new(Some(tx))),
        };
        let cancel = request.cancel.clone();
        let prepared = Prepared {
            request,
            path,
            skill: true,
            arguments: serde_json::json!({}),
            context: ToolContext {
                agent_id: "agent:test/a:latest".into(),
                resource_id: resource.to_string(),
                call_id: "call".into(),
                turn: None,
                project_root: root.path().into(),
                temp_dir: PathBuf::new(),
            },
            activity,
            config: ToolPlugin::default(),
            started: false,
            finished: false,
        };
        let caller = tokio::spawn(supervise(prepared));
        tokio::task::yield_now().await;
        caller.abort();
        let _ = caller.await;
        assert!(cancel.is_cancelled());
        let result = tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_ok() || result.is_err_and(|e| e.kind == ToolErrorKind::Cancelled));
        scope.wait_all().await.unwrap();
        assert_eq!(scope.activity_count(), 0);
    }
}
