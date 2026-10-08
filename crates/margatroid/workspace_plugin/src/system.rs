use std::sync::Arc;

use agent_image_loader_plugin::{AgentImage, LoadAgentImageResult};
use agent_plugin::{
    Agent, AgentControl, AgentControlKind, AgentControlReply, AgentCreateReply, AgentCreateRequest,
    AgentInitializationCompleted, AgentMemoryHandle, AgentModelInfo,
};
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use lua_runtime_plugin::LuaProgram;
use margatroid_types::{ResourceId, RouteAgentTurnAbort, RouteMclCommand, StartWorkspace};
use memory_plugin::AgentMemory;
use resource_id_plugin::WorldResourceIdExt;

use crate::handler::{create_workspace, route_mcl_command, stop_workspace_entity};
use crate::{
    ReloadWorkspace, ReloadWorkspaceResult, StartWorkspaceResult, StopWorkspace,
    StopWorkspaceByReference, StopWorkspaceByReferenceResult, StopWorkspaceResult, Workspace,
    WorkspaceAgentState, WorkspaceError, WorkspaceErrorKind, WorkspaceRegistry, WorldWorkspaceExt,
};

pub(crate) fn begin_workspace_command_system(world: &mut World) {
    let starts = world
        .event_reader::<StartWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in starts {
        let result = create_workspace(world, &request.definition);
        world.send_event(StartWorkspaceResult {
            id: request.id,
            result,
        });
    }

    let stops = world
        .event_reader::<StopWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in stops {
        let result = stop_workspace_entity(world, request.workspace);
        match result {
            Ok(()) => world
                .get_resource_mut::<WorkspaceRegistry>()
                .unwrap()
                .stopping
                .get_mut(&request.workspace)
                .unwrap()
                .replies
                .push(crate::types::WorkspaceStopReply::Stop(request)),
            Err(error) => world.send_event(StopWorkspaceResult {
                id: request.id,
                workspace: request.workspace,
                result: Err(error),
            }),
        }
    }

    let references = world
        .event_reader::<StopWorkspaceByReference>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in references {
        let result = world
            .entity_by_resource_id(&request.workspace.id)
            .map_err(WorkspaceError::from)
            .and_then(|workspace| {
                let matches_root = world
                    .get_component::<Workspace>(workspace)
                    .is_some_and(|value| value.project_root() == request.workspace.project_root);
                if !matches_root {
                    return Err(WorkspaceError::new(
                        WorkspaceErrorKind::WorkspaceMismatch,
                        "workspace reference does not match the active project",
                    ));
                }
                stop_workspace_entity(world, workspace).map(|_| workspace)
            });
        match result {
            Ok(workspace) => world
                .get_resource_mut::<WorkspaceRegistry>()
                .unwrap()
                .stopping
                .get_mut(&workspace)
                .unwrap()
                .replies
                .push(crate::types::WorkspaceStopReply::Reference(request)),
            Err(error) => world.send_event(StopWorkspaceByReferenceResult {
                id: request.id,
                workspace: request.workspace,
                result: Err(error),
            }),
        }
    }

    let reloads = world
        .event_reader::<ReloadWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in reloads {
        match stop_workspace_entity(world, request.workspace) {
            Ok(()) => world
                .get_resource_mut::<WorkspaceRegistry>()
                .unwrap()
                .stopping
                .get_mut(&request.workspace)
                .unwrap()
                .replies
                .push(crate::types::WorkspaceStopReply::Reload(request)),
            Err(error) => world.send_event(ReloadWorkspaceResult {
                id: request.id,
                previous: request.workspace,
                result: Err(error),
            }),
        }
    }
}

pub(crate) fn collect_workspace_stop_system(world: &mut World) {
    let workspaces = world
        .get_resource::<WorkspaceRegistry>()
        .map(|registry| registry.stopping.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    for workspace in workspaces {
        crate::handler::stop_workspace_agents(world, workspace);
        let agents = world.get_resource::<WorkspaceRegistry>().unwrap().stopping[&workspace]
            .agents
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for entity in agents {
            let drained = world.get_component::<Agent>(entity).is_none_or(|agent| {
                agent.lua.vm_id.is_none()
                    && agent.lua.request_id.is_none()
                    && matches!(
                        agent.lifecycle,
                        agent_plugin::AgentLifecycleState::Stopped
                            | agent_plugin::AgentLifecycleState::Failed
                    )
                    && agent.tools.pending.is_empty()
                    && agent.inference.pending.is_empty()
            });
            if drained {
                resource_library_plugin::release_resource_owner(world, entity);
                if world.is_alive(entity) {
                    world.despawn(entity);
                }
            }
        }
        let registry = world.get_resource::<WorkspaceRegistry>().unwrap();
        let drained = registry.stopping[&workspace]
            .agents
            .iter()
            .all(|entity| !world.is_alive(*entity))
            && !registry
                .pending_agents
                .values()
                .any(|(owner, _, _)| *owner == workspace);
        if !drained {
            continue;
        }
        let state = world
            .get_resource_mut::<WorkspaceRegistry>()
            .unwrap()
            .stopping
            .remove(&workspace)
            .unwrap();
        world
            .get_resource_mut::<WorkspaceRegistry>()
            .unwrap()
            .workspaces
            .retain(|entity| *entity != workspace);
        if let Some(routes) =
            world.get_resource_mut::<inference_plugin::WorkspaceModelRoutesRegistry>()
        {
            routes.remove(workspace);
        }
        world.despawn(workspace);
        for reply in state.replies {
            match reply {
                crate::types::WorkspaceStopReply::Stop(request) => {
                    world.send_event(StopWorkspaceResult {
                        id: request.id,
                        workspace,
                        result: Ok(()),
                    })
                }
                crate::types::WorkspaceStopReply::Reference(request) => {
                    world.send_event(StopWorkspaceByReferenceResult {
                        id: request.id,
                        workspace: request.workspace,
                        result: Ok(()),
                    })
                }
                crate::types::WorkspaceStopReply::Reload(request) => {
                    let result = create_workspace(world, &request.definition);
                    world.send_event(ReloadWorkspaceResult {
                        id: request.id,
                        previous: workspace,
                        result,
                    });
                }
            }
        }
    }
}

pub(crate) fn route_agent_turn_abort_system(world: &mut World) {
    let events = world
        .event_reader::<RouteAgentTurnAbort>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let workspace = match world.entity_by_resource_id(&event.workspace.id) {
            Ok(entity) => entity,
            Err(error) => {
                tracing::warn!(%error, "cannot route turn abort to workspace");
                continue;
            }
        };
        let agent = match event.agent.as_ref() {
            Some(id) => match world.entity_by_resource_id(id) {
                Ok(entity) => Some(entity),
                Err(error) => {
                    tracing::warn!(%error, "cannot route turn abort to agent");
                    continue;
                }
            },
            None => world.workspace_manager(workspace),
        };
        if let Some(agent) = agent {
            let (sender, _) = tokio::sync::oneshot::channel();
            world.send_event(AgentControl {
                id: event.id,
                agent,
                control: AgentControlKind::AbortTurn,
                reply: AgentControlReply::new(sender),
            });
        }
    }
}

pub(crate) fn route_mcl_command_system(world: &mut World) {
    let commands = world
        .event_reader::<RouteMclCommand>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for command in commands {
        if let Err(error) = route_mcl_command(world, command.clone()) {
            let _ = command.reply.send(Err(error.to_string()));
        }
    }
}

pub(crate) fn collect_mcl_command_response_system(world: &mut World) {
    let pending = world
        .get_resource_mut::<WorkspaceRegistry>()
        .map(|registry| std::mem::take(&mut registry.pending_mcl_commands))
        .unwrap_or_default();
    for (id, (reply, mut receiver)) in pending {
        match receiver.try_recv() {
            Ok(result) => {
                let result = result.map_err(|error| error.to_string()).and_then(|value| {
                    mcl_plugin::command_value_to_json(value).map_err(|error| error.to_string())
                });
                let _ = reply.send(result);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                world
                    .get_resource_mut::<WorkspaceRegistry>()
                    .map(|registry| {
                        registry.pending_mcl_commands.insert(id, (reply, receiver));
                    });
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                let _ = reply.send(Err("MCL command reply was closed".to_owned()));
            }
        }
    }
}

pub(crate) fn collect_agent_image_system(world: &mut World) {
    let events = world
        .event_reader::<LoadAgentImageResult>()
        .into_iter()
        .map(|event| {
            (
                event.id.clone(),
                event.reference.clone(),
                event.result.clone(),
            )
        })
        .collect::<Vec<_>>();
    for (event_id, reference, image_result) in events {
        let route = world
            .get_resource_mut::<WorkspaceRegistry>()
            .and_then(|registry| registry.pending_images.remove(&event_id));
        let Some((workspace, name)) = route else {
            continue;
        };
        let workspace_name = world
            .get_component::<Workspace>(workspace)
            .map(|value| value.definition().name.clone())
            .unwrap_or_default();
        let image = match image_result {
            Ok(image) => image,
            Err(error) => {
                tracing::error!(workspace = %workspace_name, agent = %name, error = %error, "agent image load failed");
                if let Some(value) = world.get_component_mut::<Workspace>(workspace) {
                    value.states.insert(
                        name,
                        WorkspaceAgentState::Failed {
                            error: WorkspaceError::new(
                                WorkspaceErrorKind::AgentImageLoadFailed,
                                error.to_string(),
                            ),
                        },
                    );
                }
                continue;
            }
        };
        let Some(configuration) = world.get_component::<Workspace>(workspace).cloned() else {
            continue;
        };
        let Some(definition) = configuration
            .definition()
            .agents
            .iter()
            .find(|agent| agent.name == name && agent.image == reference)
            .cloned()
        else {
            continue;
        };
        let Some(image_data) = world.get_component::<AgentImage>(image).cloned() else {
            continue;
        };
        let base_lua = image_data.base_driver();
        let image_root = world
            .get_resource::<WorkspaceRegistry>()
            .map(|registry| {
                registry
                    .agent_images_root
                    .join(definition.image.scope())
                    .join(definition.image.name())
                    .join(definition.image.tag())
            })
            .unwrap_or_default();
        let model = AgentModelInfo {
            provider: "openai-compatible".into(),
            model: image_data.model().model().into(),
            context_window_tokens: 1_000_000,
        };
        let memory_path = definition.memory_path.clone().unwrap_or_else(|| {
            configuration
                .definition()
                .project_root
                .join(".margatroid")
                .join("workspaces")
                .join(&configuration.definition().name)
                .join("memory")
                .join(&definition.name)
                .join("memory.sql")
        });
        let Ok(memory) = AgentMemory::open(memory_path) else {
            tracing::error!(workspace = %workspace_name, agent = %name, "agent memory could not be opened");
            if let Some(value) = world.get_component_mut::<Workspace>(workspace) {
                value.states.insert(
                    name,
                    WorkspaceAgentState::Failed {
                        error: WorkspaceError::new(
                            WorkspaceErrorKind::MemorySetupFailed,
                            "agent memory could not be opened",
                        ),
                    },
                );
            }
            continue;
        };
        let request_id = format!("create-{}", event_id);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        world
            .get_resource_mut::<WorkspaceRegistry>()
            .map(|registry| {
                registry.pending_agents.insert(
                    request_id.clone(),
                    (workspace, definition.name.clone(), receiver),
                );
            });
        world.send_event(AgentCreateRequest {
            id: request_id,
            agent_id: definition.id,
            workspace_id: workspace,
            image_entity: image,
            base_lua: LuaProgram {
                source: base_lua.source().to_owned(),
                origin: base_lua.origin().display().to_string(),
                entry: None,
                libraries: lua_runtime_plugin::LuaStandardLibraries::Minimal,
            },
            project_root: configuration.project_root().to_path_buf(),
            image_root: image_root.clone(),
            home_root: configuration.definition().project_root.clone(),
            model,
            memory: AgentMemoryHandle::new(Arc::new(memory)),
            token_usage: Default::default(),
            image_dependencies: Arc::from(image_data.dependencies()),
            reply: AgentCreateReply::new(sender),
        });
    }
    let pending = world
        .get_resource_mut::<WorkspaceRegistry>()
        .map(|registry| std::mem::take(&mut registry.pending_agents))
        .unwrap_or_default();
    for (id, (workspace, name, mut receiver)) in pending {
        let workspace_name = world
            .get_component::<Workspace>(workspace)
            .map(|value| value.definition().name.clone())
            .unwrap_or_default();
        match receiver.try_recv() {
            Ok(Ok(agent)) => {
                tracing::info!(workspace = %workspace_name, agent = %name, "agent created");
                if let Some(index) = world.get_component_mut::<Workspace>(workspace) {
                    index.agents.insert(name.clone(), agent);
                    index
                        .states
                        .insert(name, WorkspaceAgentState::Ready { agent });
                }
            }
            Ok(Err(error)) => {
                tracing::error!(workspace = %workspace_name, agent = %name, error = %error, "agent create failed");
                if let Some(index) = world.get_component_mut::<Workspace>(workspace) {
                    index.states.insert(
                        name,
                        WorkspaceAgentState::Failed {
                            error: WorkspaceError::new(
                                WorkspaceErrorKind::AgentCreateFailed,
                                error.to_string(),
                            ),
                        },
                    );
                }
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                world
                    .get_resource_mut::<WorkspaceRegistry>()
                    .map(|registry| {
                        registry
                            .pending_agents
                            .insert(id, (workspace, name, receiver));
                    });
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }
}

pub(crate) fn collect_agent_initialization_system(world: &mut World) {
    let events = world
        .event_reader::<AgentInitializationCompleted>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let Some(agent) = world.get_component::<Agent>(event.agent).cloned() else {
            continue;
        };
        let workspace = agent.info.workspace_id;
        let Some(agent_id) = world
            .get_component::<ResourceId>(event.agent)
            .map(|id| id.as_ref().clone())
        else {
            continue;
        };
        let Some(value) = world.get_component_mut::<Workspace>(workspace) else {
            continue;
        };
        let agent_name = value
            .definition()
            .agents
            .iter()
            .find(|definition| definition.id == agent_id)
            .map(|definition| definition.name.clone());
        if let Some(agent_name) = agent_name {
            tracing::info!(workspace = %value.definition().name, agent = %agent_name, "agent initialization completed");
            value.agents.insert(agent_name.clone(), event.agent);
            value.states.insert(
                agent_name,
                WorkspaceAgentState::Ready { agent: event.agent },
            );
        }
    }
}
