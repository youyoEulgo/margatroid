use std::collections::BTreeMap;
use std::sync::Arc;

use agent_plugin::{AgentControl, AgentControlKind, AgentControlReply};
use app_runtime_plugin::WorldEventExt;
use core_plugin::{Entity, World};
use margatroid_types::{ResourceId, RouteMclCommand, WorkspaceDefinition};
use mcl_plugin::{MclCommandId, MclCommandReply, MclCommandRequest};
use resource_id_plugin::WorldResourceIdExt;

use crate::{Workspace, WorkspaceError, WorkspaceErrorKind, WorkspaceRegistry};

pub(crate) fn stop_workspace_entity(
    world: &mut World,
    workspace: Entity,
) -> Result<(), WorkspaceError> {
    if !world.is_alive(workspace) {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceNotAlive,
            "workspace is not alive",
        ));
    }
    let agents = world
        .get_component::<Workspace>(workspace)
        .map(|value| value.iter().map(|(_, entity)| entity).collect::<Vec<_>>())
        .unwrap_or_default();
    for agent in agents {
        let (sender, _) = tokio::sync::oneshot::channel();
        world.send_event(AgentControl {
            id: format!("stop-{:?}", agent),
            agent,
            control: AgentControlKind::Stop,
            reply: AgentControlReply::new(sender),
        });
        if world.is_alive(agent) {
            world.despawn(agent);
        }
    }
    if let Some(registry) = world.get_resource_mut::<WorkspaceRegistry>() {
        registry.workspaces.retain(|entity| *entity != workspace);
        registry
            .pending_images
            .retain(|_, (owner, _)| *owner != workspace);
        registry
            .pending_agents
            .retain(|_, (owner, _, _)| *owner != workspace);
    }
    if let Some(routes) = world.get_resource_mut::<inference_plugin::WorkspaceModelRoutesRegistry>()
    {
        routes.remove(workspace);
    }
    world.despawn(workspace);
    Ok(())
}

pub(crate) fn create_workspace(
    world: &mut World,
    definition: &WorkspaceDefinition,
) -> Result<Entity, WorkspaceError> {
    if world.entity_by_resource_id(&definition.id).is_ok() {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::DuplicateWorkspace,
            "workspace already exists",
        ));
    }
    let entity = world.spawn();
    world.insert_component(entity, definition.id.clone());
    let mut workspace = Workspace {
        definition: Arc::new(definition.clone()),
        project_root: Arc::new(definition.project_root.clone()),
        manager_name: definition.manager.clone(),
        agents: BTreeMap::new(),
        states: BTreeMap::new(),
    };
    for agent in &definition.agents {
        workspace
            .states
            .insert(agent.name.clone(), crate::WorkspaceAgentState::Creating);
        let image_request_id = format!("workspace-agent-{:?}-{}", entity, agent.name);
        world
            .get_resource_mut::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed")
            .pending_images
            .insert(image_request_id.clone(), (entity, agent.name.clone()));
        world.send_event(agent_image_loader_plugin::LoadAgentImage {
            id: image_request_id,
            reference: agent.image.clone(),
        });
    }
    world.insert_component(entity, workspace);
    world
        .get_resource_mut::<WorkspaceRegistry>()
        .expect("WorkspacePlugin is not installed")
        .workspaces
        .push(entity);
    Ok(entity)
}

pub(crate) fn route_mcl_command(
    world: &mut World,
    command: RouteMclCommand,
) -> Result<(), WorkspaceError> {
    let workspace = world
        .entity_by_resource_id(&command.workspace.id)
        .map_err(|_| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotAlive,
                "workspace is not alive",
            )
        })?;
    let workspace_data = world.get_component::<Workspace>(workspace).ok_or_else(|| {
        WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceNotAlive,
            "workspace component is missing",
        )
    })?;
    let agent = match command.agent.clone() {
        Some(agent_id) => world.entity_by_resource_id(&agent_id).map_err(|_| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotReady,
                "target agent is not alive",
            )
        })?,
        None => workspace_data.manager().ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotReady,
                "workspace manager is missing",
            )
        })?,
    };
    let agent_id = world
        .get_component::<ResourceId>(agent)
        .cloned()
        .ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotReady,
                "agent identity is missing",
            )
        })?;
    let command_id = MclCommandId::new(command.id.clone()).map_err(|_| {
        WorkspaceError::new(
            WorkspaceErrorKind::InvalidRequest,
            "MCL command id is invalid",
        )
    })?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    world.send_event(MclCommandRequest {
        id: command_id,
        agent_id,
        command: command.command,
        binding: command.binding,
        reply: MclCommandReply::new(sender),
    });
    world
        .get_resource_mut::<WorkspaceRegistry>()
        .ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotAlive,
                "WorkspacePlugin is not installed",
            )
        })?
        .pending_mcl_commands
        .insert(command.id, (command.reply, receiver));
    Ok(())
}
