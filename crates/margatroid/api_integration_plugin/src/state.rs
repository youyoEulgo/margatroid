use api_plugin::{WebSocketMessageSend, WebSocketMessageTarget};
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_protocol::{
    AgentHistoryDto, BackendStateDto, HistoryMessageDto, ResourceRefDto, ServerEvent,
};
use memory_plugin::AgentMemory;
use workspace_plugin::{WorkspaceAgents, WorldWorkspaceExt};

use crate::identity::workspace_info;

pub(crate) fn sync_frontend_state_system(world: &mut World, frontend_type: &str) {
    let state = match backend_state(world) {
        Ok(state) => state,
        Err(error) => {
            tracing::warn!(error = %error, "frontend state sync failed");
            return;
        }
    };
    world.send_event(WebSocketMessageSend {
        target: WebSocketMessageTarget::Type(frontend_type.into()),
        message: ServerEvent::StateSync { state },
    });
}

fn backend_state(world: &World) -> Result<BackendStateDto, String> {
    let mut workspaces = world
        .workspaces()
        .into_iter()
        .filter_map(|workspace| workspace_info(world, workspace).map(|info| (workspace, info)))
        .collect::<Vec<_>>();
    workspaces.sort_by(|left, right| {
        left.1
            .project_root
            .cmp(&right.1.project_root)
            .then_with(|| left.1.name.cmp(&right.1.name))
    });
    let mut workspace_infos = Vec::with_capacity(workspaces.len());
    let mut histories = Vec::new();
    for (workspace, info) in workspaces {
        let agents = world
            .get_component::<WorkspaceAgents>(workspace)
            .ok_or_else(|| format!("workspace `{}` does not have an Agent index", info.name))?;
        for (name, agent) in agents.iter() {
            let memory = world.get_component::<AgentMemory>(agent).ok_or_else(|| {
                format!(
                    "Agent `{name}` in workspace `{}` does not have memory",
                    info.name
                )
            })?;
            let messages = memory
                .history_messages()
                .map_err(|error| {
                    format!(
                        "Agent `{name}` history in workspace `{}` could not be read: {error}",
                        info.name
                    )
                })?
                .into_iter()
                .map(|message| HistoryMessageDto {
                    sequence: message.sequence,
                    turn_id: message.turn_id,
                    message: message.message,
                    resources: message
                        .resources
                        .into_iter()
                        .map(|resource| ResourceRefDto {
                            provider: resource.provider().to_owned(),
                            name: resource.name().to_string(),
                        })
                        .collect(),
                    created_at_ms: message.created_at_ms,
                })
                .collect();
            histories.push(AgentHistoryDto {
                workspace: info.reference(),
                agent: name.to_owned(),
                messages,
            });
        }
        workspace_infos.push(info);
    }
    Ok(BackendStateDto {
        workspaces: workspace_infos,
        histories,
    })
}
