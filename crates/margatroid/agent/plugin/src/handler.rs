use crate::{types::*, AgentInstalled};
use address::{resolve_file, Address, ParsedAddress};
use agent_core::*;
use agent_driver_core::{read_driver, AgentDriverInput, AgentDriverReadLimits};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use closure_plugin::WorldClosureExt;
use core_plugin::{Entity, World};
use futures_util::{future::join_all, FutureExt};
use inference_core::{
    ActiveInference, AgentInferenceConfig, InferenceHandle, InferenceParameters, InitInference,
    ModelRoutes,
};
use lua_runtime_core::*;
use mcl_core::{MclDrain, MclQueue};
use prompt_resource_core::Prompts;
use sandbox_resource_core::{Sandboxes, SelectedSandbox};
use std::{
    collections::BTreeMap,
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
    name: &str,
    parameters: &AgentStartParameters,
) -> Result<String, AgentError> {
    let component = world
        .get_component::<Workspace>(workspace)
        .ok_or_else(|| error(AgentErrorKind::InvalidRequest, "Workspace is missing"))?;
    if !matches!(
        component.state,
        WorkspaceState::Starting | WorkspaceState::Running | WorkspaceState::Restarting
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
    let locator = ParsedAddress::parse(&parameters.driver)
        .map_err(|e| error(AgentErrorKind::InvalidRequest, e))?;
    if locator.kind != "driver" {
        return Err(error(
            AgentErrorKind::InvalidRequest,
            "expected a driver file locator",
        ));
    }
    Ok(scope.to_string())
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
        id,
        agent,
        workspace: world.get_component::<Agent>(agent).unwrap().workspace,
        state,
        parameters,
        reply,
        events: world.event_sender(),
        world: world.closure_sender(&installed.schedule),
        driver_root: installed.driver_root.clone(),
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
    if component.workspace != workspace || component.state != state {
        return Err(error(
            AgentErrorKind::StateRejected,
            "Agent lifecycle state or Workspace does not match",
        ));
    }
    Ok(())
}
fn cancel_runtime(world: &mut World, agent: Entity) {
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
    if let Some(vm) = world.get_component::<DriverLuaVm>(agent) {
        vm.cancel.cancel();
        vm.wake
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .close();
    }
}
pub(crate) fn handle_start_agent(world: &mut World, request: StartAgent) {
    let Some(reply) = claim(&request.reply) else {
        return;
    };
    let prepared = (|| {
        validate_request(&request.id, &request.name)?;
        let scope =
            validate_parameters(world, request.workspace, &request.name, &request.parameters)?;
        if world
            .query_with::<Agent>()
            .result()
            .into_iter()
            .any(|entity| {
                world.get_component::<Agent>(entity).is_some_and(|agent| {
                    agent.workspace == request.workspace && agent.name == request.name
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
        world.insert_component(agent, address);
        world.insert_component(
            agent,
            Agent {
                name: request.name.clone(),
                workspace: request.workspace,
                state: AgentState::Starting,
                turn: None,
            },
        );
        if let Err(error) = world.workspace_insert_agent(request.workspace, agent) {
            world.despawn(agent);
            return Err(AgentError::new(
                AgentErrorKind::InvalidRequest,
                error.to_string(),
            ));
        }
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
        if !matches!(component.state, AgentState::Idle | AgentState::Working) {
            return Err(error(
                AgentErrorKind::StateRejected,
                "Agent is not Idle or Working",
            ));
        }
        let component = world.get_component_mut::<Agent>(request.agent).unwrap();
        component.state = AgentState::Stopping;
        component.turn = None;
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
        validate_parameters(world, request.workspace, &request.name, &request.parameters)?;
        let matches: Vec<_> = world
            .query_with::<Agent>()
            .result()
            .into_iter()
            .filter(|entity| {
                world.get_component::<Agent>(*entity).is_some_and(|agent| {
                    agent.workspace == request.workspace && agent.name == request.name
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
            world.get_component::<Agent>(agent).unwrap().state,
            AgentState::Idle | AgentState::Working
        ) {
            return Err(error(
                AgentErrorKind::StateRejected,
                "Agent is not Idle or Working",
            ));
        }
        let component = world.get_component_mut::<Agent>(agent).unwrap();
        component.state = AgentState::Restarting;
        component.turn = None;
        cancel_runtime(world, agent);
        Ok(context(
            world,
            request.id.clone(),
            agent,
            AgentState::Restarting,
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
    if let Some(agent) = world.get_component_mut::<Agent>(event.agent) {
        agent.state = AgentState::Stopping;
        agent.turn = None;
        cancel_runtime(world, event.agent);
        tracing::error!(request=%event.id,agent=?event.agent,error=%event.error,"Agent exception closed new admission; cleanup is not confirmed");
    }
}
fn despawn_clean(world: &mut World, agent: Entity) {
    if let Some(component) = world.get_component::<Agent>(agent) {
        let workspace = component.workspace;
        let _ = world.workspace_remove_agent(workspace, agent);
    }
    world.despawn(agent);
}
fn finish_result(world: &mut World, agent: Entity, state: AgentState) -> Result<(), AgentError> {
    let component = world.get_component_mut::<Agent>(agent).ok_or_else(|| {
        error(
            AgentErrorKind::AgentMissing,
            "Agent result has lost its host",
        )
    })?;
    if component.state != state {
        return Err(error(
            AgentErrorKind::StateRejected,
            "stale Agent lifecycle result",
        ));
    }
    component.state = AgentState::Idle;
    component.turn = None;
    Ok(())
}
fn failed_result(world: &mut World, failure: FlowError, state: AgentState) {
    tracing::error!(request=%failure.id,error=%failure.error,cleanup_completed=failure.cleanup_completed,"Agent lifecycle failed");
    if failure.cleanup_completed {
        if let Some(agent) = failure.agent {
            if world
                .get_component::<Agent>(agent)
                .is_some_and(|component| component.state == state)
            {
                despawn_clean(world, agent);
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
                .is_some_and(|component| component.state == AgentState::Stopping)
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
                finish_result(world, output.agent, AgentState::Restarting).map(|()| output.agent);
            tracing::debug!(request=%output.id,"Agent restart finished");
            deliver(&output.reply, result);
        }
        Err(error) => failed_result(world, error, AgentState::Restarting),
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
) -> Result<
    (
        AgentDriverInput,
        AgentStorage,
        agent_core::AgentDriverSource,
    ),
    AgentError,
> {
    let parameters = context.parameters.clone().ok_or_else(|| {
        error(
            AgentErrorKind::InvalidRequest,
            "Agent startup parameters are missing",
        )
    })?;
    let root = context.driver_root.clone();
    tokio::task::spawn_blocking(move || {
        let path = resolve_file(&root, &parameters.driver)
            .map_err(|e| error(AgentErrorKind::StartFailed, e))?;
        let driver = read_driver(&path, &AgentDriverReadLimits::default())
            .map_err(|e| error(AgentErrorKind::StartFailed, e))?;
        reject_symlinks(&parameters.database_path)?;
        reject_symlinks(&parameters.settings_path)?;
        let storage = open_storage(
            &parameters.database_path,
            &parameters.settings_path,
            &driver.default_settings,
        )
        .map_err(|e| error(AgentErrorKind::StartFailed, e))?;
        Ok((
            driver,
            storage,
            agent_core::AgentDriverSource {
                address: parameters.driver,
                manifest_path: path,
            },
        ))
    })
    .await
    .map_err(|e| error(AgentErrorKind::StartFailed, e))?
}
fn entity_value(entity: Entity) -> LuaValue {
    LuaValue::Object(BTreeMap::from([
        ("index".into(), LuaValue::Integer(i64::from(entity.index()))),
        (
            "generation".into(),
            LuaValue::Integer(i64::from(entity.generation())),
        ),
    ]))
}
async fn install_runtime(
    context: &FlowContext,
    driver: &AgentDriverInput,
    storage: AgentStorage,
    source: agent_core::AgentDriverSource,
) -> Result<LuaEnvironment, AgentError> {
    let agent = context.agent;
    let workspace = context.workspace;
    let state = context.state;
    let manifest = driver.manifest.clone();
    let parameters = context.parameters.clone().unwrap();
    let (environment, finished) = with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        if world.get_component::<DriverLuaVm>(agent).is_some() {
            return Err(error(
                AgentErrorKind::CleanupFailed,
                "old Driver VM still exists",
            ));
        }
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
        let route = world
            .get_resource::<ModelRoutes>()
            .and_then(|routes| routes.get(&manifest.inference.model))
            .ok_or_else(|| {
                error(
                    AgentErrorKind::StartFailed,
                    "Driver model has no configured ModelRoute",
                )
            })?;
        let tokens = i64::try_from(route.context_window_tokens).map_err(|_| {
            error(
                AgentErrorKind::StartFailed,
                "model context window exceeds Lua integer range",
            )
        })?;
        let model = LuaValue::Object(BTreeMap::from([
            (
                "id".into(),
                LuaValue::String(manifest.inference.model.clone()),
            ),
            ("model".into(), LuaValue::String(route.model.clone())),
            ("provider".into(), LuaValue::String(route.provider.clone())),
            ("api_type".into(), LuaValue::String(route.api_type.clone())),
            ("context_window_tokens".into(), LuaValue::Integer(tokens)),
            (
                "thinking".into(),
                route
                    .thinking
                    .clone()
                    .map(LuaValue::String)
                    .unwrap_or(LuaValue::Nil),
            ),
            (
                "reasoning_effort".into(),
                route
                    .reasoning_effort
                    .clone()
                    .map(LuaValue::String)
                    .unwrap_or(LuaValue::Nil),
            ),
        ]));
        let inference = InferenceParameters {
            model: manifest.inference.model,
            temperature: manifest.inference.temperature,
            max_output_tokens: manifest.inference.max_output_tokens,
            top_p: manifest.inference.top_p,
            stop: manifest.inference.stop,
        };
        let name = world.get_component::<Agent>(agent).unwrap().name.clone();
        let info = LuaValue::Object(BTreeMap::from([
            ("name".into(), LuaValue::String(name)),
            ("entity".into(), entity_value(agent)),
            ("workspace".into(), entity_value(workspace)),
            (
                "project_root".into(),
                LuaValue::String(parameters.project_root.to_string_lossy().into_owned()),
            ),
            (
                "home_root".into(),
                LuaValue::String(parameters.home_root.to_string_lossy().into_owned()),
            ),
            ("model".into(), model),
            (
                "inference".into(),
                LuaValue::from_json(
                    serde_json::to_value(&inference)
                        .map_err(|e| error(AgentErrorKind::StartFailed, e))?,
                )
                .map_err(|e| error(AgentErrorKind::StartFailed, e))?,
            ),
        ]));
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
        world.insert_component(agent, source);
        world.insert_component(
            agent,
            MclQueue {
                allowed_imports: manifest.dependencies,
                ..MclQueue::default()
            },
        );
        world.insert_component(
            agent,
            AgentInferenceConfig {
                parameters: inference,
            },
        );
        world.insert_component(agent, Tools::default());
        world.insert_component(agent, Prompts::default());
        world.insert_component(agent, Sandboxes::default());
        world.insert_component(agent, SelectedSandbox::default());
        Ok((
            LuaEnvironment {
                globals: BTreeMap::from([("agent_info".into(), LuaBinding::Value(info))]),
                modules: BTreeMap::new(),
            },
            finished,
        ))
    })
    .await?;
    if let Some(finished) = finished {
        finished.wait_finished().await.map_err(|e| {
            error(
                AgentErrorKind::CleanupFailed,
                format!("previous inference task ended with error: {e}"),
            )
        })?;
    }
    Ok(environment)
}
async fn start_steps(context: &FlowContext) -> Result<(), StartFailure> {
    let (driver, storage, source) = prepare(context).await?;
    let environment = install_runtime(context, &driver, storage, source).await?;
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
    let (sender, receiver) = oneshot::channel();
    context.events.send_event(StartDriverLuaVm {
        owner: context.agent,
        workspace: context.workspace,
        program: LuaProgram {
            source: driver.program,
            origin: driver.origin,
            libraries: LuaStandardLibraries::Minimal,
        },
        environment,
        reply: Arc::new(Mutex::new(Some(sender))),
    });
    let instance = match receiver.await {
        Ok(Ok(instance)) => instance,
        Ok(Err(e)) => {
            let uncertain = e.kind == LuaRuntimeErrorKind::CleanupFailed;
            return Err(StartFailure {
                error: error(AgentErrorKind::StartFailed, e),
                uncertain,
            });
        }
        Err(e) => {
            return Err(StartFailure {
                error: error(
                    AgentErrorKind::ReplyClosed,
                    format!("VM start reply closed: {e}"),
                ),
                uncertain: true,
            });
        }
    };
    with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        if instance.owner != agent
            || !world
                .get_component::<DriverLuaVm>(agent)
                .is_some_and(|vm| vm.instance == instance)
        {
            return Err(error(
                AgentErrorKind::CleanupFailed,
                "Driver startup result does not match the current VM instance",
            ));
        }
        Ok(())
    })
    .await
    .map_err(|error| StartFailure {
        error,
        uncertain: true,
    })?;
    Ok(())
}
struct StopSnapshot {
    instance: Option<DriverLuaVmInstance>,
    storage: Option<AgentStorage>,
    inference: Option<InferenceHandle>,
    tasks: Vec<ToolTaskHandle>,
    drain: Option<MclDrain>,
}
async fn stop_runtime(context: &FlowContext, require_vm: bool) -> Result<(), AgentError> {
    let agent = context.agent;
    let workspace = context.workspace;
    let state = context.state;
    let snapshot = with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        cancel_runtime(world, agent);
        let instance = world
            .get_component::<DriverLuaVm>(agent)
            .map(|vm| vm.instance.clone());
        if require_vm && instance.is_none() {
            return Err(error(
                AgentErrorKind::CleanupFailed,
                "Driver VM is missing without a confirmed stop result",
            ));
        }
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
        let storage = world.get_component::<AgentStorage>(agent).cloned();
        Ok(StopSnapshot {
            instance,
            storage,
            inference,
            tasks,
            drain,
        })
    })
    .await?;
    let StopSnapshot {
        instance,
        storage,
        inference,
        tasks,
        drain,
    } = snapshot;
    let cleanup_instance = instance.as_ref().map(|instance| instance.start_id.clone());
    let vm = async {
        if let Some(instance) = instance {
            let (sender, receiver) = oneshot::channel();
            context.events.send_event(StopDriverLuaVm {
                instance,
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            receiver
                .await
                .map_err(|e| {
                    error(
                        AgentErrorKind::ReplyClosed,
                        format!("VM stop reply closed without cleanup proof: {e}"),
                    )
                })?
                .map_err(|e| error(AgentErrorKind::CleanupFailed, e))?;
        }
        Ok::<(), AgentError>(())
    };
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
    let (vm, inference, tasks, drain) = tokio::join!(vm, inference, tasks, drain);
    let mut failures = Vec::new();
    for result in [vm, inference, drain].into_iter().chain(tasks) {
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    if !failures.is_empty() {
        return Err(error(AgentErrorKind::CleanupFailed, failures.join("; ")));
    }
    if let (Some(storage), Some(instance)) = (storage, cleanup_instance) {
        tokio::task::spawn_blocking(move || {
            execute_storage(&storage, StorageOperation::DropTemporaryBlocks { instance })
        })
        .await
        .map_err(|failure| error(AgentErrorKind::CleanupFailed, failure))?
        .map_err(|failure| error(AgentErrorKind::CleanupFailed, failure))?;
    }
    with_world(context, move |world| {
        check_agent(world, agent, workspace, state)?;
        if world.get_component::<DriverLuaVm>(agent).is_some() {
            return Err(error(
                AgentErrorKind::CleanupFailed,
                "Driver stop was acknowledged while its component remains",
            ));
        }
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
        world.remove_component::<agent_core::AgentDriverSource>(agent);
        Ok(())
    })
    .await
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
    stop_runtime(&context, true)
        .await
        .map_err(|e| failed(&context, e, false))?;
    Ok(StopOutput {
        id: context.id,
        agent: context.agent,
        reply: context.reply,
    })
}
async fn restart_flow(context: FlowContext) -> Result<RestartOutput, RestartError> {
    stop_runtime(&context, true)
        .await
        .map_err(|e| failed(&context, e, false))?;
    startup(&context).await?;
    Ok(RestartOutput {
        id: context.id,
        agent: context.agent,
        reply: context.reply,
    })
}
async fn guarded_start(context: FlowContext) -> Result<StartOutput, StartError> {
    let retained = context.clone();
    AssertUnwindSafe(start_flow(context))
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
        })
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
    AssertUnwindSafe(restart_flow(context))
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
        })
}
