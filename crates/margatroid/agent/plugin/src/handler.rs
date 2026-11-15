use crate::{types::*, AgentInstalled};
use address::Address;
use agent_core::WorldAgentExt;
use agent_core::*;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use closure_plugin::WorldClosureExt;
use core_plugin::{Entity, World};
use futures_util::{future::join_all, FutureExt};
use inference_core::{ActiveInference, InferenceHandle, InitInference};
use mcl_core::{MclDrain, MclQueue};
use prompt_resource_core::Prompts;
use sandbox_resource_core::{Sandboxes, SelectedSandbox};
use std::{
    panic::AssertUnwindSafe,
    path::{Component, Path},
    sync::{Arc, Mutex},
};
use storage_core::{execute_storage, open_storage, AgentStorage, StorageOperation};
use tokio::sync::oneshot;
use tool_core::{InitToolTasks, ToolTaskHandle, ToolTasks};
use tool_resource_core::Tools;
use workspace_core::{Workspace, WorkspaceExecutor, WorkspaceState, WorldWorkspaceExt};

fn error(kind: AgentErrorKind, message: impl std::fmt::Display) -> AgentError {
    AgentError::new(kind, message.to_string())
}
fn deliver(reply: &Reply, result: Result<Entity, AgentError>) {
    if let Some(sender) = reply
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take()
    {
        let _ = sender.send(result);
    }
}
fn claim(reply: &Reply) -> Option<Reply> {
    reply
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take()
        .map(|sender| Arc::new(Mutex::new(Some(sender))))
}
fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}
fn validate_request(id: &str, name: &str) -> Result<(), AgentError> {
    if !valid_text(id)
        || !valid_text(name)
        || name.len() > 128
        || matches!(name, "." | "..")
        || name.chars().any(|c| matches!(c, '/' | '\\' | ':'))
    {
        return Err(error(
            AgentErrorKind::InvalidRequest,
            "invalid request id or member name",
        ));
    }
    Ok(())
}
fn validate_parameters(
    world: &World,
    workspace: Entity,
    owner: Entity,
    name: &str,
    parameters: &AgentStartParameters,
) -> Result<String, AgentError> {
    let component = world
        .get_component::<Workspace>(workspace)
        .ok_or_else(|| error(AgentErrorKind::InvalidRequest, "Workspace is missing"))?;
    if !matches!(
        component.state,
        WorkspaceState::Starting | WorkspaceState::Running
    ) {
        return Err(error(
            AgentErrorKind::StateRejected,
            "Workspace is not accepting member initialization",
        ));
    }
    let executor = world
        .get_component::<WorkspaceExecutor>(workspace)
        .ok_or_else(|| {
            error(
                AgentErrorKind::InvalidRequest,
                "Workspace executor is missing",
            )
        })?;
    if executor.entry.is_closed() {
        return Err(error(
            AgentErrorKind::StateRejected,
            "Workspace task admission is closed",
        ));
    }
    let home = component.definition_path.parent().ok_or_else(|| {
        error(
            AgentErrorKind::InvalidRequest,
            "Workspace definition path has no home",
        )
    })?;
    let scope = home
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or_else(|| {
            error(
                AgentErrorKind::InvalidRequest,
                "Workspace home has no valid name",
            )
        })?;
    let home = if owner == workspace {
        home.to_path_buf()
    } else {
        let parent = world.get_component::<Agent>(owner).ok_or_else(|| {
            error(
                AgentErrorKind::InvalidRequest,
                "owner is neither Workspace nor Agent",
            )
        })?;
        if parent.workspace() != workspace
            || matches!(parent.state(), AgentState::Stopping | AgentState::Error)
        {
            return Err(error(
                AgentErrorKind::StateRejected,
                "parent Agent is closing",
            ));
        }
        world
            .get_component::<agent_core::AgentResources>(owner)
            .and_then(|source| source.parameters.settings_path.parent())
            .ok_or_else(|| {
                error(
                    AgentErrorKind::InvalidRequest,
                    "parent Agent resources missing",
                )
            })?
            .to_path_buf()
    };
    let directory = home.join("agents").join(name);
    if parameters.database_path != directory.join("state.sqlite")
        || parameters.settings_path != directory.join("setting.toml")
    {
        return Err(error(
            AgentErrorKind::InvalidRequest,
            "Agent database and settings must use the managed Workspace member home",
        ));
    }
    for path in [
        &parameters.database_path,
        &parameters.settings_path,
        &parameters.initial_settings_path,
        &parameters.ui_path,
        &parameters.project_root,
        &parameters.home_root,
    ] {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(error(
                AgentErrorKind::InvalidRequest,
                "Agent directories must be absolute paths without traversal",
            ));
        }
    }
    if owner == workspace {
        Ok(scope.to_owned())
    } else {
        use sha2::Digest;
        let address = world.get_component::<Address>(owner).ok_or_else(|| {
            error(
                AgentErrorKind::InvalidRequest,
                "parent Agent address missing",
            )
        })?;
        Ok(format!(
            "child-{:x}",
            sha2::Sha256::digest(address.as_str().as_bytes())
        ))
    }
}
fn context(
    world: &World,
    id: String,
    agent: Entity,
    state: AgentState,
    parameters: Option<AgentStartParameters>,
    reply: Reply,
) -> FlowContext {
    let installed = world.get_resource::<AgentInstalled>().unwrap();
    FlowContext {
        startup: world
            .get_component::<StartupBarrier>(agent)
            .expect("Agent startup barrier missing")
            .0
            .clone(),
        id,
        agent,
        workspace: world.get_component::<Agent>(agent).unwrap().workspace(),
        state,
        parameters,
        reply,
        events: world.event_sender(),
        world: world.closure_sender(&installed.schedule),
    }
}
fn check_agent(
    world: &World,
    agent: Entity,
    workspace: Entity,
    state: AgentState,
) -> Result<(), AgentError> {
    let component = world
        .get_component::<Agent>(agent)
        .ok_or_else(|| error(AgentErrorKind::AgentMissing, "Agent entity is missing"))?;
    if component.workspace() != workspace || component.state() != state {
        return Err(error(
            AgentErrorKind::StateRejected,
            "Agent lifecycle state or Workspace does not match",
        ));
    }
    Ok(())
}
fn cancel_runtime(world: &mut World, agent: Entity) {
    if let Some(slot) = world.get_component::<lua_runtime_core::AgentWaitSlot>(agent) {
        slot.0.lock().unwrap_or_else(|e| e.into_inner()).close();
    }
    if let Some(queue) = world.get_component_mut::<MclQueue>(agent) {
        queue.accepting = false;
        queue.cancel.cancel();
    }
    if let Some(active) = world.get_component::<ActiveInference>(agent) {
        if let Some(handle) = &active.current {
            handle.cancel();
        }
    }
    if let Some(tasks) = world.get_component::<ToolTasks>(agent) {
        for (_, handle) in tasks.list() {
            handle.cancel();
        }
    }
}
pub(crate) fn handle_start_agent(world: &mut World, request: StartAgent) {
    let Some(reply) = claim(&request.reply) else {
        return;
    };
    let prepared = (|| {
        validate_request(&request.id, &request.name)?;
        let scope = validate_parameters(
            world,
            request.workspace,
            request.owner,
            &request.name,
            &request.parameters,
        )?;
        if world
            .query_with::<Agent>()
            .result()
            .into_iter()
            .any(|entity| {
                world.get_component::<Agent>(entity).is_some_and(|agent| {
                    agent.workspace() == request.workspace
                        && agent.name() == &request.name
                        && world
                            .get_component::<agent_core::AgentOwner>(entity)
                            .is_some_and(|owner| owner.0 == request.owner)
                })
            })
        {
            return Err(error(
                AgentErrorKind::StateRejected,
                "Agent member name is already active",
            ));
        }
        let address = Address::new(&format!("agent:{scope}/{}:latest", request.name))
            .map_err(|e| error(AgentErrorKind::InvalidRequest, e))?;
        if world
            .query_with::<Address>()
            .result()
            .into_iter()
            .any(|entity| world.get_component::<Address>(entity) == Some(&address))
        {
            return Err(error(
                AgentErrorKind::StateRejected,
                "Agent address is already active",
            ));
        }
        let agent = world.spawn();
        world.insert_component(
            agent,
            StartupBarrier(Arc::new(StartupCompletion::default())),
        );
        world.insert_component(agent, address);
        world.insert_component(agent, agent_core::AgentOwner(request.owner));
        world.insert_component(
            agent,
            AgentResources {
                parameters: request.parameters.clone(),
            },
        );
        world.insert_component(
            agent,
            agent_core::Agent::new(
                request.name.clone(),
                request.workspace,
                AgentState::Starting,
                None,
                world.get_resource::<AgentInstalled>().unwrap().sync.clone(),
            ),
        );
        if let Err(error) = world.workspace_insert_agent(request.workspace, agent) {
            world.despawn(agent);
            return Err(AgentError::new(
                AgentErrorKind::InvalidRequest,
                error.to_string(),
            ));
        }
        let publication = world.get_component::<Agent>(agent).unwrap().updates();
        let waits = publication.notify(world, agent, agent_core::AgentUpdateKind::Changed);
        assert!(waits.is_empty());
        Ok(context(
            world,
            request.id.clone(),
            agent,
            AgentState::Starting,
            Some(request.parameters.clone()),
            reply.clone(),
        ))
    })();
    match prepared {
        Err(error) => deliver(&reply, Err(error)),
        Ok(context) => {
            let schedule = world
                .get_resource::<AgentInstalled>()
                .unwrap()
                .schedule
                .clone();
            world.send_async_closure(&schedule, move || guarded_start(context));
        }
    }
}
pub(crate) fn handle_stop_agent(world: &mut World, request: StopAgent) {
    let Some(reply) = claim(&request.reply) else {
        return;
    };
    let prepared = (|| {
        if !valid_text(&request.id) {
            return Err(error(
                AgentErrorKind::InvalidRequest,
                "invalid stop request id",
            ));
        }
        let component = world
            .get_component::<Agent>(request.agent)
            .ok_or_else(|| error(AgentErrorKind::AgentMissing, "Agent is missing"))?;
        if component.state() == AgentState::Stopping {
            return Err(error(
                AgentErrorKind::StateRejected,
                "Agent is already stopping",
            ));
        }
        world.agent_transition(request.agent, AgentState::Stopping, None)?;
        cancel_runtime(world, request.agent);
        Ok(context(
            world,
            request.id.clone(),
            request.agent,
            AgentState::Stopping,
            None,
            reply.clone(),
        ))
    })();
    match prepared {
        Err(error) => deliver(&reply, Err(error)),
        Ok(context) => {
            let schedule = world
                .get_resource::<AgentInstalled>()
                .unwrap()
                .schedule
                .clone();
            world.send_async_closure(&schedule, move || guarded_stop(context));
        }
    }
}
pub(crate) fn handle_restart_agent(world: &mut World, request: RestartAgent) {
    let Some(reply) = claim(&request.reply) else {
        return;
    };
    let prepared = (|| {
        validate_request(&request.id, &request.name)?;
        validate_parameters(
            world,
            request.workspace,
            request.workspace,
            &request.name,
            &request.parameters,
        )?;
        let matches: Vec<_> = world
            .query_with::<Agent>()
            .result()
            .into_iter()
            .filter(|entity| {
                world.get_component::<Agent>(*entity).is_some_and(|agent| {
                    agent.workspace() == request.workspace && agent.name() == &request.name
                })
            })
            .collect();
        let agent = match matches.as_slice() {
            [agent] => *agent,
            [] => {
                return Err(error(
                    AgentErrorKind::AgentMissing,
                    "Agent member is missing",
                ));
            }
            _ => {
                return Err(error(
                    AgentErrorKind::InvalidRequest,
                    "Agent member name is ambiguous",
                ));
            }
        };
        if world
            .get_component::<ToolTasks>(agent)
            .is_some_and(|tasks| !tasks.list().is_empty())
        {
            return Err(error(
                AgentErrorKind::UnfinishedTasks,
                "Agent has unfinished tool tasks",
            ));
        }
        if !matches!(
            world.get_component::<Agent>(agent).unwrap().state(),
            AgentState::Running | AgentState::Starting | AgentState::Error
        ) {
            return Err(error(
                AgentErrorKind::StateRejected,
                "Agent is not Idle or Working",
            ));
        }
        world.insert_component(
            agent,
            StartupBarrier(Arc::new(StartupCompletion::default())),
        );
        world.agent_transition(agent, AgentState::Stopping, None)?;
        cancel_runtime(world, agent);
        Ok(context(
            world,
            request.id.clone(),
            agent,
            AgentState::Stopping,
            Some(request.parameters.clone()),
            reply.clone(),
        ))
    })();
    match prepared {
        Err(error) => deliver(&reply, Err(error)),
        Ok(context) => {
            let schedule = world
                .get_resource::<AgentInstalled>()
                .unwrap()
                .schedule
                .clone();
            world.send_async_closure(&schedule, move || guarded_restart(context));
        }
    }
}
pub(crate) fn handle_agent_exception(world: &mut World, event: AgentException) {
    if let Some(agent) = world.get_component::<Agent>(event.agent) {
        if !matches!(agent.state(), AgentState::Starting | AgentState::Running) {
            return;
        }
        let context = agent.updates();
        let _ = agent_core::update_agent(
            world,
            event.agent,
            &context,
            agent_core::AgentChange::Fault(event.error.clone()),
        );
        cancel_runtime(world, event.agent);
        tracing::error!(request=%event.id,agent=?event.agent,error=%event.error,"Agent exception closed new admission; cleanup is not confirmed");
    }
}
fn despawn_clean(world: &mut World, agent: Entity) {
    if let Some(component) = world.get_component::<Agent>(agent) {
        let workspace = component.workspace();
        let _ = world.workspace_remove_agent(workspace, agent);
    }
    agent_core::remove_agent(world, agent);
}
fn finish_result(world: &mut World, agent: Entity, state: AgentState) -> Result<(), AgentError> {
    let component = world.get_component_mut::<Agent>(agent).ok_or_else(|| {
        error(
            AgentErrorKind::AgentMissing,
            "Agent result has lost its host",
        )
    })?;
    if component.state() != state {
        return Err(error(
            AgentErrorKind::StateRejected,
            "stale Agent lifecycle result",
        ));
    }
    Ok(())
}
fn failed_result(world: &mut World, failure: FlowError, state: AgentState) {
    tracing::error!(request=%failure.id,error=%failure.error,cleanup_completed=failure.cleanup_completed,"Agent lifecycle failed");
    if let Some(agent) = failure.agent {
        let expected = failure.state.unwrap_or(state);
        if world
            .get_component::<Agent>(agent)
            .is_some_and(|component| component.state() == expected)
        {
            if failure.cleanup_completed {
                despawn_clean(world, agent);
            } else {
                let context = world.get_component::<Agent>(agent).unwrap().updates();
                let _ = agent_core::update_agent(
                    world,
                    agent,
                    &context,
                    agent_core::AgentChange::Fault(failure.error.clone()),
                );
                cancel_runtime(world, agent);
            }
        }
    }
    if let Some(reply) = failure.reply {
        deliver(&reply, Err(failure.error));
    }
}
pub(crate) fn handle_start_result(world: &mut World, result: Result<StartOutput, StartError>) {
    match result {
        Ok(output) => {
            let result =
                finish_result(world, output.agent, AgentState::Starting).map(|()| output.agent);
            tracing::debug!(request=%output.id,"Agent start finished");
            deliver(&output.reply, result);
        }
        Err(error) => failed_result(world, error, AgentState::Starting),
    }
}
pub(crate) fn handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>) {
    match result {
        Ok(output) => {
            let result = if world
                .get_component::<Agent>(output.agent)
                .is_some_and(|component| component.state() == AgentState::Stopping)
            {
                despawn_clean(world, output.agent);
                Ok(output.agent)
            } else {
                Err(error(
                    AgentErrorKind::StateRejected,
                    "stale Agent stop result",
                ))
            };
            tracing::debug!(request=%output.id,"Agent stop finished");
            deliver(&output.reply, result);
        }
        Err(error) => failed_result(world, error, AgentState::Stopping),
    }
}
pub(crate) fn handle_restart_result(
    world: &mut World,
    result: Result<RestartOutput, RestartError>,
) {
    match result {
        Ok(output) => {
            let result =
                finish_result(world, output.agent, AgentState::Starting).map(|()| output.agent);
            tracing::debug!(request=%output.id,"Agent restart finished");
            deliver(&output.reply, result);
        }
        Err(error) => failed_result(world, error, AgentState::Starting),
    }
}
async fn with_world<T: Send + 'static>(
    context: &FlowContext,
    operation: impl FnOnce(&mut World) -> Result<T, AgentError> + Send + 'static,
) -> Result<T, AgentError> {
    let (sender, receiver) = oneshot::channel();
    context.world.send(move |world| {
        let _ = sender.send(operation(world));
    });
    receiver.await.map_err(|_| {
        error(
            AgentErrorKind::CleanupFailed,
            "Agent World closure reply was lost",
        )
    })?
}
fn failed(context: &FlowContext, error: AgentError, cleanup_completed: bool) -> FlowError {
    FlowError {
        id: context.id.clone(),
        agent: Some(context.agent),
        reply: Some(context.reply.clone()),
        error,
        cleanup_completed,
        state: Some(context.state),
    }
}
fn reject_symlinks(path: &Path) -> Result<(), AgentError> {
    let mut current = std::path::PathBuf::new();
    for part in path.components() {
        current.push(part.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(error(
                    AgentErrorKind::StartFailed,
                    "managed Agent state paths cannot contain symbolic links",
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(AgentErrorKind::StartFailed, e)),
        }
    }
    Ok(())
}
async fn prepare(
    context: &FlowContext,
) -> Result<(AgentStorage, agent_core::AgentResources), AgentError> {
    let parameters = context.parameters.clone().ok_or_else(|| {
        error(
            AgentErrorKind::InvalidRequest,
            "Agent startup parameters are missing",
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let default_settings = if parameters.settings_path.exists() {
            String::new()
        } else {
            agent_driver_core::read_initial_settings(
                &parameters.initial_settings_path,
                agent_driver_core::AgentDriverReadLimits::default().settings_bytes,
            )
            .map_err(|error| self::error(AgentErrorKind::StartFailed, error))?
        };
        reject_symlinks(&parameters.database_path)?;
        reject_symlinks(&parameters.settings_path)?;
        let storage = open_storage(
            &parameters.database_path,
            &parameters.settings_path,
            &default_settings,
        )
        .map_err(|e| error(AgentErrorKind::StartFailed, e))?;
        Ok((storage, agent_core::AgentResources { parameters }))
    })
    .await
    .map_err(|e| error(AgentErrorKind::StartFailed, e))?
}
async fn install_runtime(
    context: &FlowContext,
    storage: AgentStorage,
    source: agent_core::AgentResources,
) -> Result<(), AgentError> {
    let agent = context.agent;
    let workspace = context.workspace;
    let state = context.state;
    let (finished, observers) = with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        if world
            .get_component::<ActiveInference>(agent)
            .and_then(|active| active.current.as_ref())
            .is_some_and(|handle| !handle.is_finished())
        {
            return Err(error(
                AgentErrorKind::CleanupFailed,
                "cannot replace an active inference handle",
            ));
        }
        let finished = world
            .get_component_mut::<ActiveInference>(agent)
            .and_then(|active| active.current.take());
        if let Some(queue) = world.get_component::<MclQueue>(agent) {
            if !queue.commands.is_empty() || queue.pending.is_some() {
                return Err(error(
                    AgentErrorKind::CleanupFailed,
                    "old MCL commands have not drained",
                ));
            }
        }
        world.insert_component(agent, storage);
        world.remove_component::<lua_runtime_core::LuaAgentMonitor>(agent);
        world.insert_component(agent, agent_core::AgentSession::new());
        world.insert_component(agent, lua_runtime_core::AgentWaitSlot::default());
        world.insert_component(agent, source);
        world.insert_component(agent, MclQueue::default());
        world.insert_component(
            agent,
            Tools::new(
                world
                    .get_resource::<tool_resource_core::ToolResourceSyncBinding>()
                    .expect("tool resource sync required")
                    .for_owner(agent),
            ),
        );
        world.insert_component(agent, Prompts::default());
        world.insert_component(
            agent,
            Sandboxes::new(
                world
                    .get_resource::<sandbox_resource_core::SandboxSyncBinding>()
                    .expect("sandbox sync required")
                    .for_owner(agent),
            ),
        );
        world.insert_component(
            agent,
            SelectedSandbox::new(
                None,
                world
                    .get_resource::<sandbox_resource_core::SandboxSelectionSyncBinding>()
                    .expect("sandbox selection sync required")
                    .for_owner(agent),
            ),
        );
        if let Some(tasks) = world.get_component::<ToolTasks>(agent) {
            if !tasks.list().is_empty() {
                return Err(error(
                    AgentErrorKind::CleanupFailed,
                    "previous tool tasks remain",
                ));
            }
            tasks.close_sync();
        }
        world.remove_component::<ToolTasks>(agent);
        let observers = agent_core::notify_agent_runtime(world, agent, false)?;
        Ok((finished, observers))
    })
    .await?;
    wait_observers(observers).await?;
    if let Some(finished) = finished {
        finished.wait_finished().await.map_err(|e| {
            error(
                AgentErrorKind::CleanupFailed,
                format!("previous inference task ended with error: {e}"),
            )
        })?;
    }
    Ok(())
}
async fn start_steps(context: &FlowContext) -> Result<(), StartFailure> {
    let (storage, source) = prepare(context).await?;
    install_runtime(context, storage, source).await?;
    let (tool_sender, tool_receiver) = oneshot::channel();
    let (inference_sender, inference_receiver) = oneshot::channel();
    let agent = context.agent;
    let workspace = context.workspace;
    let state = context.state;
    with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        world.send_event(InitToolTasks {
            agent,
            reply: Arc::new(Mutex::new(Some(tool_sender))),
        });
        world.send_event(InitInference {
            agent,
            reply: Arc::new(Mutex::new(Some(inference_sender))),
        });
        Ok(())
    })
    .await?;
    let (tools, inference) = tokio::join!(tool_receiver, inference_receiver);
    let uncertain = tools.is_err() || inference.is_err();
    let mut failures = Vec::new();
    match tools {
        Ok(Ok(())) => {}
        Ok(Err(e)) => failures.push(e.to_string()),
        Err(e) => failures.push(format!("tool initialization reply closed: {e}")),
    }
    match inference {
        Ok(Ok(())) => {}
        Ok(Err(e)) => failures.push(e.to_string()),
        Err(e) => failures.push(format!("inference initialization reply closed: {e}")),
    }
    if !failures.is_empty() {
        return Err(StartFailure {
            error: error(
                if uncertain {
                    AgentErrorKind::ReplyClosed
                } else {
                    AgentErrorKind::StartFailed
                },
                failures.join("; "),
            ),
            uncertain,
        });
    }
    let agent = context.agent;
    let workspace = context.workspace;
    let state = context.state;
    with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        if world.get_component::<ToolTasks>(agent).is_none()
            || world.get_component::<ActiveInference>(agent).is_none()
        {
            return Err(error(
                AgentErrorKind::StartFailed,
                "runtime initialization acknowledged without required components",
            ));
        }
        if world
            .get_component::<ActiveInference>(agent)
            .unwrap()
            .current
            .is_some()
        {
            return Err(error(
                AgentErrorKind::CleanupFailed,
                "runtime initialization left an inference active",
            ));
        }
        Ok(())
    })
    .await?;

    Ok(())
}
struct StopSnapshot {
    storage: Option<AgentStorage>,
    session: Option<String>,
    inference: Option<InferenceHandle>,
    tasks: Vec<ToolTaskHandle>,
    drain: Option<MclDrain>,
}
async fn stop_runtime(context: &FlowContext, _require_vm: bool) -> Result<(), AgentError> {
    let agent = context.agent;
    let workspace = context.workspace;
    let state = context.state;
    let children = with_world(context, move |world| {
        Ok(world
            .query_with::<agent_core::AgentOwner>()
            .result()
            .into_iter()
            .filter(|entity| {
                world
                    .get_component::<agent_core::AgentOwner>(*entity)
                    .is_some_and(|owner| owner.0 == agent)
                    && world.get_component::<Agent>(*entity).is_some()
            })
            .collect::<Vec<_>>())
    })
    .await?;
    for child in children {
        let (tx, rx) = oneshot::channel();
        context.events.send_event(StopAgent {
            id: format!("{}:child:{}", context.id, child.index()),
            agent: child,
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        rx.await
            .map_err(|e| error(AgentErrorKind::ReplyClosed, e))??;
    }
    let snapshot = with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        cancel_runtime(world, agent);
        let inference = world
            .get_component_mut::<ActiveInference>(agent)
            .and_then(|active| active.current.take());
        let tasks = world
            .get_component::<ToolTasks>(agent)
            .map(|tasks| tasks.list().into_iter().map(|(_, handle)| handle).collect())
            .unwrap_or_default();
        let drain = world
            .get_component::<MclQueue>(agent)
            .map(|queue| queue.drain.clone());
        Ok(StopSnapshot {
            storage: world.get_component::<AgentStorage>(agent).cloned(),
            session: world
                .get_component::<agent_core::AgentSession>(agent)
                .map(|session| session.0.clone()),
            inference,
            tasks,
            drain,
        })
    })
    .await?;
    let StopSnapshot {
        storage,
        session,
        inference,
        tasks,
        drain,
    } = snapshot;
    let inference = async move {
        if let Some(handle) = inference {
            handle.wait_finished().await.map_err(|e| {
                error(
                    AgentErrorKind::CleanupFailed,
                    format!("inference join failed: {e}"),
                )
            })?;
        }
        Ok::<(), AgentError>(())
    };
    let tasks = async move {
        join_all(tasks.into_iter().map(|handle| async move {
            handle
                .finish()
                .await
                .map_err(|e| error(AgentErrorKind::CleanupFailed, e))
        }))
        .await
    };
    let drain = async move {
        if let Some(drain) = drain {
            drain
                .wait()
                .await
                .map_err(|e| error(AgentErrorKind::CleanupFailed, e))?;
        }
        Ok::<(), AgentError>(())
    };
    let (inference, tasks, drain) = tokio::join!(inference, tasks, drain);
    let mut failures = Vec::new();
    for result in [inference, drain].into_iter().chain(tasks) {
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    if !failures.is_empty() {
        return Err(error(AgentErrorKind::CleanupFailed, failures.join("; ")));
    }
    if let (Some(storage), Some(instance)) = (storage, session) {
        tokio::task::spawn_blocking(move || {
            execute_storage(&storage, StorageOperation::DropTemporaryBlocks { instance })
        })
        .await
        .map_err(|e| error(AgentErrorKind::CleanupFailed, e))?
        .map_err(|e| error(AgentErrorKind::CleanupFailed, e))?;
    }
    let observers = with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        world.remove_component::<lua_runtime_core::LuaAgentMonitor>(agent);
        if let Some(active) = world.get_component_mut::<ActiveInference>(agent) {
            if active
                .current
                .as_ref()
                .is_some_and(|handle| !handle.is_finished())
            {
                return Err(error(
                    AgentErrorKind::CleanupFailed,
                    "inference activity remains after stop",
                ));
            }
            active.current.take();
        }
        if world
            .get_component::<ToolTasks>(agent)
            .is_some_and(|tasks| !tasks.list().is_empty())
        {
            return Err(error(
                AgentErrorKind::CleanupFailed,
                "tool activity remains after stop",
            ));
        }
        if world
            .get_component::<MclQueue>(agent)
            .is_some_and(|queue| !queue.commands.is_empty() || queue.pending.is_some())
        {
            return Err(error(
                AgentErrorKind::CleanupFailed,
                "MCL commands have not fully drained",
            ));
        }
        world.remove_component::<agent_core::AgentResources>(agent);
        agent_core::notify_agent_runtime(world, agent, true)
    })
    .await?;
    wait_observers(observers).await
}
async fn startup(context: &FlowContext) -> Result<(), FlowError> {
    match start_steps(context).await {
        Ok(()) => Ok(()),
        Err(failure) => {
            let cleanup = stop_runtime(context, false).await;
            let completed = cleanup.is_ok() && !failure.uncertain;
            let error = match cleanup {
                Ok(()) => failure.error,
                Err(cleanup) => error(
                    AgentErrorKind::CleanupFailed,
                    format!("{}; startup cleanup failed: {cleanup}", failure.error),
                ),
            };
            Err(failed(context, error, completed))
        }
    }
}
async fn start_flow(context: FlowContext) -> Result<StartOutput, StartError> {
    startup(&context).await?;
    Ok(StartOutput {
        id: context.id,
        agent: context.agent,
        reply: context.reply,
    })
}
async fn stop_flow(context: FlowContext) -> Result<StopOutput, StopError> {
    context.startup.wait().await;
    stop_runtime(&context, true)
        .await
        .map_err(|e| failed(&context, e, false))?;
    Ok(StopOutput {
        id: context.id,
        agent: context.agent,
        reply: context.reply,
    })
}
async fn restart_flow(mut context: FlowContext) -> Result<RestartOutput, RestartError> {
    stop_runtime(&context, true)
        .await
        .map_err(|e| failed(&context, e, false))?;
    let agent = context.agent;
    let workspace = context.workspace;
    with_world(&context, move |world| {
        check_agent(world, agent, workspace, AgentState::Stopping)?;
        world.agent_transition(agent, AgentState::Starting, None)
    })
    .await
    .map_err(|e| failed(&context, e, false))?;
    context.state = AgentState::Starting;
    startup(&context).await?;
    Ok(RestartOutput {
        id: context.id,
        agent: context.agent,
        reply: context.reply,
    })
}
async fn guarded_start(context: FlowContext) -> Result<StartOutput, StartError> {
    let retained = context.clone();
    let result = AssertUnwindSafe(start_flow(context))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(failed(
                &retained,
                error(
                    AgentErrorKind::CleanupFailed,
                    "Agent start flow panicked; cleanup is not confirmed",
                ),
                false,
            ))
        });
    retained.startup.finish();
    result
}
async fn guarded_stop(context: FlowContext) -> Result<StopOutput, StopError> {
    let retained = context.clone();
    AssertUnwindSafe(stop_flow(context))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(failed(
                &retained,
                error(
                    AgentErrorKind::CleanupFailed,
                    "Agent stop flow panicked; cleanup is not confirmed",
                ),
                false,
            ))
        })
}
async fn guarded_restart(context: FlowContext) -> Result<RestartOutput, RestartError> {
    let retained = context.clone();
    let result = AssertUnwindSafe(restart_flow(context))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(failed(
                &retained,
                error(
                    AgentErrorKind::CleanupFailed,
                    "Agent restart flow panicked; cleanup is not confirmed",
                ),
                false,
            ))
        });
    retained.startup.finish();
    result
}

async fn wait_observers(
    observers: Vec<agent_core::AgentObservationWait>,
) -> Result<(), AgentError> {
    let errors = futures_util::future::join_all(observers)
        .await
        .into_iter()
        .filter_map(Result::err)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(error(AgentErrorKind::CleanupFailed, errors.join("; ")))
    }
}
