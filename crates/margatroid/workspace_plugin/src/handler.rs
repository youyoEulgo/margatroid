use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::*;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::{Entity, World};
use margatroid_types::agent_driver_events::{LoadAgentDriver, LoadAgentDriverResult};
use margatroid_types::agent_events::{
    AgentStartDefinition, StartAgent, StartAgentResult, StopAgent, StopAgentResult,
};
use margatroid_types::resource_components::AgentDriver;
use margatroid_types::workspace_execution::spawn_workspace_execution;
use resource_id_plugin::{LibraryComponent, ResourceSet, WorldResourceExt};

fn request_id() -> String {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let session = SESSION.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must follow Unix epoch")
            .as_nanos();
        format!("{}:{nanos}", std::process::id())
    });
    format!(
        "workspace-operation-{session}:{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn error(kind: WorkspaceErrorKind, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::new(kind, message)
}
fn record(target: &mut Option<WorkspaceError>, error: WorkspaceError) {
    if target.is_none() {
        *target = Some(error);
    } else {
        tracing::warn!(%error, "additional workspace operation failure");
    }
}
fn validate_definition(request: &StartWorkspace) -> Result<(), WorkspaceError> {
    let definition = &request.definition;
    if request.id.is_empty() {
        return Err(error(
            WorkspaceErrorKind::InvalidRequest,
            "request ID is empty",
        ));
    }
    if definition.id.resource_type() != "workspace"
        || definition.project_root.as_os_str().is_empty()
    {
        return Err(error(
            WorkspaceErrorKind::InvalidDefinition,
            "invalid workspace ID or project root",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for agent in &definition.agents {
        if agent.id.resource_type() != "agent"
            || agent.driver.resource_type() != "driver"
            || agent.name.is_empty()
            || !ids.insert(&agent.id)
            || !names.insert(&agent.name)
            || agent.home_root.as_os_str().is_empty()
            || agent.history_path.as_os_str().is_empty()
        {
            return Err(error(
                WorkspaceErrorKind::InvalidDefinition,
                "invalid or duplicate member definition",
            ));
        }
    }
    if !definition.agents.is_empty() && !names.contains(&definition.manager) {
        return Err(error(
            WorkspaceErrorKind::InvalidDefinition,
            "manager is not an initial member",
        ));
    }
    Ok(())
}

pub(crate) fn handle_start_workspace(world: &mut World, request: StartWorkspace) {
    let result = start(world, &request);
    if let Err(error) = result {
        world.send_event(StartWorkspaceResult {
            id: request.id,
            result: Err(error),
        });
    }
}
fn start(world: &mut World, request: &StartWorkspace) -> Result<(), WorkspaceError> {
    validate_definition(request)?;
    let existing = request
        .definition
        .id
        .find_entities(world)
        .map_err(|value| {
            error(
                WorkspaceErrorKind::ResourceResolutionFailed,
                value.to_string(),
            )
        })?;
    if !existing.is_empty() {
        return Err(error(
            WorkspaceErrorKind::DuplicateWorkspace,
            "workspace address is already occupied",
        ));
    }
    let (entry, close) = spawn_workspace_execution()
        .map_err(|value| error(WorkspaceErrorKind::ExecutionStartFailed, value.to_string()))?;
    let mut state = WorkspaceStartState {
        request_id: request.id.clone(),
        pending_drivers: BTreeMap::new(),
        driver_members: BTreeMap::new(),
        pending_agents: BTreeMap::new(),
        error: None,
    };
    for member in &request.definition.agents {
        state
            .driver_members
            .entry(member.driver.clone())
            .or_default()
            .push(member.id.clone());
    }
    let mut loads = Vec::new();
    for driver in state.driver_members.keys() {
        let id = request_id();
        state.pending_drivers.insert(id.clone(), driver.clone());
        loads.push((id, driver.clone()));
    }
    let entity = world.spawn();
    let component = Workspace::new(
        request.definition.clone(),
        WorkspaceState::Starting(state),
        entry,
        close.clone(),
    );
    if let Err(value) = world.insert_library(entity, component) {
        world.despawn(entity);
        world.spawn_async_service(async move {
            let _ = close.close().await;
        });
        return Err(error(
            WorkspaceErrorKind::ResourceResolutionFailed,
            value.to_string(),
        ));
    }
    for (id, resource_id) in loads {
        world.send_event(LoadAgentDriver {
            id,
            target: entity,
            resource_id,
        });
    }
    finish_workspace_start(world, entity);
    Ok(())
}

pub(crate) fn handle_agent_driver_result(world: &mut World, event: LoadAgentDriverResult) {
    let matching = world.query_with::<Workspace>().result().into_iter().find(|entity| world.get_component::<Workspace>(*entity).is_some_and(|workspace| matches!(workspace.state(), WorkspaceState::Starting(state) if state.pending_drivers.get(&event.id) == Some(&event.resource_id))));
    let Some(entity) = matching else {
        return;
    };
    if entity != event.target {
        return;
    }
    let loaded = world
        .get_component::<ResourceSet<AgentDriver>>(entity)
        .is_some_and(|drivers| drivers.get(&event.resource_id).is_some());
    let failure = match &event.result {
        Err(value) => Some(error(
            WorkspaceErrorKind::AgentDriverLoadFailed,
            value.to_string(),
        )),
        Ok(()) if !loaded => Some(error(
            WorkspaceErrorKind::ResourceResolutionFailed,
            "driver success has no entry on target workspace",
        )),
        Ok(()) => None,
    };
    let mut starts = Vec::new();
    let workspace = world.get_component_mut::<Workspace>(entity).unwrap();
    let definition = workspace.definition().clone();
    if let WorkspaceState::Starting(state) = workspace.state_mut() {
        state.pending_drivers.remove(&event.id);
        if let Some(value) = failure {
            record(&mut state.error, value);
        } else {
            let members = state
                .driver_members
                .get(&event.resource_id)
                .cloned()
                .unwrap_or_default();
            for resource_id in members {
                if let Some(member) = definition
                    .agents
                    .iter()
                    .find(|member| member.id == resource_id)
                {
                    let id = request_id();
                    state.pending_agents.insert(id.clone(), resource_id.clone());
                    starts.push(StartAgent {
                        id,
                        resource_id,
                        workspace: entity,
                        driver: event.resource_id.clone(),
                        definition: AgentStartDefinition {
                            project_root: definition.project_root.clone(),
                            home_root: member.home_root.clone(),
                            history_path: member.history_path.clone(),
                        },
                    });
                }
            }
        }
    }
    for request in starts {
        world.send_event(request);
    }
    finish_workspace_start(world, entity);
}

pub(crate) fn handle_start_agent_result(world: &mut World, event: StartAgentResult) {
    let matching = world.query_with::<Workspace>().result().into_iter().find(|entity| world.get_component::<Workspace>(*entity).is_some_and(|workspace| match workspace.state() {
        WorkspaceState::Starting(state) => state.pending_agents.contains_key(&event.id),
        WorkspaceState::Stopping(state) => state.pending_creations.contains_key(&event.id),
        WorkspaceState::Running => true,
        _ => false,
    } && match workspace.state() { WorkspaceState::Running => workspace.has_pending_creation(&event.id), _ => true }));
    let Some(entity) = matching else {
        return;
    };
    let mut stop = None;
    let workspace = world.get_component_mut::<Workspace>(entity).unwrap();
    match workspace.state_mut() {
        WorkspaceState::Starting(state) => {
            let resource_id = state.pending_agents.remove(&event.id).unwrap();
            match event.result {
                Ok(agent) => workspace.insert_agent(resource_id, agent),
                Err(value) => {
                    if let WorkspaceState::Starting(state) = workspace.state_mut() {
                        record(
                            &mut state.error,
                            error(WorkspaceErrorKind::AgentStartFailed, value.to_string()),
                        );
                    }
                }
            }
        }
        WorkspaceState::Stopping(state) => {
            let resource_id = state.pending_creations.remove(&event.id).unwrap();
            match event.result {
                Ok(agent) => {
                    let id = request_id();
                    state
                        .pending_agents
                        .insert(id.clone(), (resource_id.clone(), agent));
                    stop = Some(StopAgent { id, agent });
                    workspace.insert_agent(resource_id, agent);
                }
                Err(value) => record(
                    &mut state.error,
                    error(WorkspaceErrorKind::AgentStartFailed, value.to_string()),
                ),
            }
        }
        WorkspaceState::Running => {
            let resource_id = workspace.consume_creation(&event.id).unwrap();
            match event.result {
                Ok(agent) => workspace.insert_agent(resource_id, agent),
                Err(value) => tracing::warn!(error = %value, "dynamic member creation failed"),
            }
        }
        _ => return,
    }
    if let Some(request) = stop {
        world.send_event(request);
    }
    finish_workspace_start(world, entity);
    finish_workspace_stop(world, entity);
}

fn finish_workspace_start(world: &mut World, entity: Entity) {
    let Some(workspace) = world.get_component_mut::<Workspace>(entity) else {
        return;
    };
    let WorkspaceState::Starting(state) = workspace.state() else {
        return;
    };
    if !state.pending_drivers.is_empty() || !state.pending_agents.is_empty() {
        return;
    }
    let id = state.request_id.clone();
    let result = match state.error.clone() {
        Some(value) => {
            workspace.close_admission();
            workspace.set_state(WorkspaceState::Failed {
                error: value.clone(),
            });
            Err(value)
        }
        None => {
            workspace.set_state(WorkspaceState::Running);
            Ok(entity)
        }
    };
    world.send_event(StartWorkspaceResult { id, result });
}

pub(crate) fn handle_stop_workspace(world: &mut World, request: StopWorkspace) {
    let Some(workspace) = world.get_component_mut::<Workspace>(request.workspace) else {
        world.send_event(StopWorkspaceResult {
            id: request.id,
            result: Err(error(
                WorkspaceErrorKind::WorkspaceNotAlive,
                "workspace does not exist",
            )),
        });
        return;
    };
    if request.id.is_empty() || !matches!(workspace.state(), WorkspaceState::Running) {
        world.send_event(StopWorkspaceResult {
            id: request.id,
            result: Err(error(
                WorkspaceErrorKind::WorkspaceNotRunning,
                "workspace is not Running or request ID is empty",
            )),
        });
        return;
    }
    workspace.close_admission();
    let mut state = WorkspaceStopState {
        request_id: request.id,
        pending_agents: BTreeMap::new(),
        pending_creations: workspace.take_pending_creations(),
        closing_execution: false,
        error: None,
    };
    let mut stops = Vec::new();
    for (resource_id, agent) in workspace.iter() {
        let id = request_id();
        state
            .pending_agents
            .insert(id.clone(), (resource_id.clone(), agent));
        stops.push(StopAgent { id, agent });
    }
    workspace.set_state(WorkspaceState::Stopping(state));
    for request in stops {
        world.send_event(request);
    }
    finish_workspace_stop(world, request.workspace);
}

pub(crate) fn handle_stop_agent_result(world: &mut World, event: StopAgentResult) {
    let matching = world.query_with::<Workspace>().result().into_iter().find(|entity| world.get_component::<Workspace>(*entity).is_some_and(|workspace| matches!(workspace.state(), WorkspaceState::Stopping(state) if state.pending_agents.contains_key(&event.id))));
    let Some(entity) = matching else {
        return;
    };
    let workspace = world.get_component_mut::<Workspace>(entity).unwrap();
    if let WorkspaceState::Stopping(state) = workspace.state_mut() {
        let (resource_id, agent) = state.pending_agents[&event.id].clone();
        if matches!(&event.result, Ok(actual) if *actual != agent) {
            return;
        }
        state.pending_agents.remove(&event.id);
        match event.result {
            Ok(_) => workspace.remove_agent(&resource_id),
            Err(value) => record(
                &mut state.error,
                error(WorkspaceErrorKind::AgentStopFailed, value.to_string()),
            ),
        }
    }
    finish_workspace_stop(world, entity);
}

fn finish_workspace_stop(world: &mut World, entity: Entity) {
    let Some(workspace) = world.get_component_mut::<Workspace>(entity) else {
        return;
    };
    let WorkspaceState::Stopping(state) = workspace.state_mut() else {
        return;
    };
    if !state.pending_agents.is_empty()
        || !state.pending_creations.is_empty()
        || state.closing_execution
    {
        return;
    }
    let request_id = state.request_id.clone();
    if let Some(value) = state.error.clone() {
        workspace.set_state(WorkspaceState::Failed {
            error: value.clone(),
        });
        world.send_event(StopWorkspaceResult {
            id: request_id,
            result: Err(value),
        });
        return;
    }
    state.closing_execution = true;
    let close = workspace.execution_close();
    let sender = world.event_sender();
    world.spawn_async_service(async move {
        let result = close
            .close()
            .await
            .map_err(|value| error(WorkspaceErrorKind::ExecutionCloseFailed, value.to_string()));
        sender.send_event(WorkspaceExecutionCloseOutput {
            workspace: entity,
            request_id,
            result,
        });
    });
}

pub(crate) fn handle_workspace_execution_closed(
    world: &mut World,
    output: WorkspaceExecutionCloseOutput,
) {
    let Some(workspace) = world.get_component_mut::<Workspace>(output.workspace) else {
        return;
    };
    if !matches!(workspace.state(), WorkspaceState::Stopping(state) if state.request_id == output.request_id && state.closing_execution)
    {
        return;
    }
    let result = match output.result {
        Ok(()) => {
            world.despawn(output.workspace);
            Ok(output.workspace)
        }
        Err(value) => {
            workspace.set_state(WorkspaceState::Failed {
                error: value.clone(),
            });
            Err(value)
        }
    };
    world.send_event(StopWorkspaceResult {
        id: output.request_id,
        result,
    });
}

pub(crate) fn handle_agent_exception_result(
    world: &mut World,
    event: margatroid_types::agent_events::AgentExceptionResult,
) {
    let Ok(entity) = world.workspace_by_id(&event.workspace) else {
        return;
    };
    if event.instance.owner != event.agent {
        return;
    }
    let workspace = world.get_component_mut::<Workspace>(entity).unwrap();
    if !workspace
        .iter()
        .any(|(id, agent)| id == &event.resource_id && agent == event.agent)
    {
        return;
    }
    if !workspace.record_exception(event.exception_id.clone()) {
        return;
    }
    if !event.cleanup_completed || event.result.is_err() {
        workspace.close_admission();
    }
    let result = event
        .result
        .map_err(|value| error(WorkspaceErrorKind::AgentStopFailed, value.to_string()));
    let original_error = event
        .error
        .map(|value| error(WorkspaceErrorKind::AgentStopFailed, value.to_string()));
    world.send_event(WorkspaceExceptionResult {
        exception_id: event.exception_id,
        workspace: entity,
        resource_id: event.workspace,
        agent: event.agent,
        agent_resource_id: event.resource_id,
        error: original_error,
        result,
    });
}

pub(crate) fn handle_workspace_exception(world: &mut World, event: WorkspaceException) {
    let result = match world.get_component_mut::<Workspace>(event.workspace) {
        None => Err(error(
            WorkspaceErrorKind::WorkspaceNotAlive,
            "exception workspace does not exist",
        )),
        Some(workspace) => {
            if workspace.resource_id() != &event.resource_id
                || !workspace
                    .iter()
                    .any(|(id, entity)| id == &event.agent_resource_id && entity == event.agent)
            {
                Err(error(
                    WorkspaceErrorKind::InvalidRequest,
                    "exception member association does not match",
                ))
            } else {
                if !workspace.record_exception(event.exception_id.clone()) {
                    return;
                }
                workspace.close_admission();
                tracing::warn!(error = %event.error, "workspace exception closes submission gate");
                Ok(())
            }
        }
    };
    world.send_event(WorkspaceExceptionResult {
        exception_id: event.exception_id,
        workspace: event.workspace,
        resource_id: event.resource_id,
        agent: event.agent,
        agent_resource_id: event.agent_resource_id,
        error: Some(event.error),
        result,
    });
}
