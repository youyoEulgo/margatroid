use std::path::PathBuf;

use api_plugin::{AgentMessageRequested, WorkspaceStartRequested};
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_protocol::WorkspaceRefDto;
use margatroid_types::{AgentMessage, AgentReference, Message, MessageIntent};
use workspace_plugin::{StartWorkspace, WorkspaceConfiguration, WorldWorkspaceExt};

pub(crate) fn handle_workspace_start_requests(world: &mut World) {
    let requests = world
        .event_reader::<WorkspaceStartRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        match request.definition.into_definition() {
            Ok(definition) => world.send_event(StartWorkspace {
                id: request.id,
                definition,
            }),
            Err(error) => tracing::warn!(
                connection = request.connection_id.get(),
                error = %error,
                "workspace request contains an invalid definition"
            ),
        }
    }
}

pub(crate) fn handle_agent_message_requests(world: &mut World) {
    let requests = world
        .event_reader::<AgentMessageRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        if let Err(error) = route_agent_message(
            world,
            request.id,
            request.workspace,
            request.agent,
            request.content,
        ) {
            tracing::warn!(
                connection = request.connection_id.get(),
                error = %error,
                "agent message was rejected"
            );
        }
    }
}

fn route_agent_message(
    world: &World,
    id: String,
    workspace: WorkspaceRefDto,
    agent: Option<String>,
    content: String,
) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("message id cannot be empty".into());
    }
    if content.trim().is_empty() {
        return Err("message content cannot be empty".into());
    }
    let project_root = PathBuf::from(&workspace.project_root);
    let workspace_entity = world
        .workspace(&project_root, &workspace.name)
        .ok_or_else(|| "workspace was not found or is not ready".to_owned())?;
    let agent_name = match agent {
        Some(name) if name.trim().is_empty() => return Err("agent name cannot be empty".into()),
        Some(name) => name,
        None => world
            .get_component::<WorkspaceConfiguration>(workspace_entity)
            .ok_or_else(|| "workspace configuration is missing".to_owned())?
            .definition()
            .manager
            .clone(),
    };
    let configuration = world
        .get_component::<WorkspaceConfiguration>(workspace_entity)
        .ok_or_else(|| "workspace configuration is missing".to_owned())?;
    let index = configuration
        .definition()
        .agents
        .iter()
        .position(|candidate| candidate.name == agent_name)
        .ok_or_else(|| format!("agent `{agent_name}` was not found in the workspace"))?;
    let agent_id = format!("{}.{}{}", workspace.name, agent_name, index);
    world.send_event(AgentMessage {
        id,
        agent: AgentReference::Id(agent_id),
        message: Message::User { content },
        intent: MessageIntent::UserWithoutToolCalls,
    });
    Ok(())
}
