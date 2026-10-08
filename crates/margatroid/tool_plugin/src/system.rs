use agent_plugin::{Agent, AgentToolPending, ToolExecutor};
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_types::{AgentFailure, AgentFailureKind};

use crate::handler;
use crate::{ToolCallRequest, ToolError, ToolErrorKind, ToolRegisterRequest, ToolRegisterResponse};

pub(crate) fn tool_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolRegisterRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        if request.id.is_empty() {
            world.send_event(ToolRegisterResponse {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                alias: request.alias,
                result: Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "resource registration request is invalid",
                )),
            });
            continue;
        }
        let resource_type = request.resource_id.resource_type();
        if matches!(
            resource_type,
            "skill" | "hook" | "sandbox" | "tool" | "prompt"
        ) {
            continue;
        }
        world.send_event(ToolRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            alias: request.alias,
            result: Err(ToolError::new(
                ToolErrorKind::ProviderMissing,
                "resource type has no built-in executor",
            )),
        });
    }
}

pub(crate) fn tool_call_route_system(world: &mut World) {
    let calls = world
        .event_reader::<margatroid_types::ToolCallEvent>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in calls {
        let result = (|| {
            if event.turn_id.is_empty() || event.call.id.is_empty() || !world.is_alive(event.agent)
            {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "tool call event is invalid",
                ));
            }
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                &event.call.arguments,
            )
            .map_err(|_| {
                ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "tool arguments must be a JSON object",
                )
            })?;
            let entry = world
                .get_component::<Agent>(event.agent)
                .and_then(|agent| agent.resources.tool_by_name(&event.call.tool_name))
                .ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::InvalidRequest,
                        "tool name is not registered for this Agent",
                    )
                })?;
            let request = ToolCallRequest {
                effect_id: event.effect_id.clone(),
                turn_id: event.turn_id.clone(),
                agent: event.agent,
                executor: entry.executor,
                resource_id: entry.resource_id.clone(),
                tool_call_id: event.call.id.clone(),
                arguments: event.call.arguments.clone(),
            };
            let agent = world
                .get_component_mut::<Agent>(event.agent)
                .ok_or_else(|| {
                    ToolError::new(ToolErrorKind::AgentNotAlive, "Agent component is missing")
                })?;
            let key = (
                event.agent,
                request.turn_id.clone(),
                request.tool_call_id.clone(),
            );
            if agent.tools.pending.contains_key(&key) {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "tool call is already pending",
                ));
            }
            agent.tools.pending.insert(
                key,
                AgentToolPending {
                    turn_id: request.turn_id.clone(),
                    tool_call_id: request.tool_call_id.clone(),
                    resource_id: request.resource_id.clone(),
                    executor: request.executor,
                },
            );
            match request.executor {
                ToolExecutor::SkillLoader => {
                    let result = handler::skill::execute_skill_call(world, &request);
                    handler::finish_tool_call(world, request, result);
                }
                ToolExecutor::Hook => {
                    let result = handler::hook::execute_hook_call(world, &request);
                    handler::finish_tool_call(world, request, result);
                }
                ToolExecutor::LuaRuntime => {
                    let result = handler::lua::prepare_lua_call(world, request.clone());
                    if let Err(error) = result {
                        handler::finish_tool_call(world, request, Err(error));
                    }
                }
            }
            Ok::<(), ToolError>(())
        })();
        if let Err(error) = result {
            world.send_event(margatroid_types::MclEffectReply {
                id: event.turn_id.clone(),
                agent: event.agent,
                result: Err(error.to_string()),
                usage: None,
            });
            world.send_event(AgentFailure {
                id: event.turn_id,
                agent: event.agent,
                kind: AgentFailureKind::Agent,
                message: error.to_string(),
            });
        }
    }
}
pub(crate) fn cancel_tool_turn_system(world: &mut World) {
    let cancellations = world
        .event_reader::<crate::CancelToolTurn>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for cancellation in cancellations {
        if let Some(agent) = world.get_component_mut::<Agent>(cancellation.agent) {
            agent.tools.pending.retain(|(entity, turn_id, _), _| {
                *entity != cancellation.agent || turn_id != &cancellation.turn_id
            });
        }
    }
}
