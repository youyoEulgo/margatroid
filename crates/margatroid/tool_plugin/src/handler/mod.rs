pub(crate) mod hook;
pub(crate) mod lua;
pub(crate) mod shell;
pub(crate) mod skill;

use agent_plugin::Agent;
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_types::{AgentMessage, Message};

use crate::{ToolCallRequest, ToolError};

pub(crate) fn finish_tool_call(
    world: &mut World,
    request: ToolCallRequest,
    result: Result<String, ToolError>,
) {
    let pending = world
        .get_component_mut::<Agent>(request.agent)
        .and_then(|agent| {
            agent.tools.pending.remove(&(
                request.agent,
                request.turn_id.clone(),
                request.tool_call_id.clone(),
            ))
        });
    if pending.is_none() {
        return;
    }
    let content = result.unwrap_or_else(|error| error.to_string());
    world.send_event(AgentMessage {
        id: request.turn_id,
        agent: request.agent,
        message: Message::Tool {
            resource_id: request.resource_id,
            tool_call_id: request.tool_call_id,
            content,
        },
        usage: None,
    });
}
