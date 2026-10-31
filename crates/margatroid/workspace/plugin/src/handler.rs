use crate::{
    checked_path, invalid, reject_symlinks, types::*, validate_definition_names, validate_name,
    WorkspaceInstalled,
};
use address::Address;
use agent_core::{Agent, AgentStartParameters, AgentState, StartAgent, StopAgent};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use closure_plugin::WorldClosureExt;
use core_plugin::{Entity, World};
use futures_util::FutureExt;
use std::{
    collections::BTreeSet,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;
use tool_core::ToolTasks;
use workspace_core::*;

fn error(kind: WorkspaceErrorKind, message: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::new(kind, message)
}
fn valid_id(id: &str) -> Result<(), WorkspaceError> {
    if id.trim().is_empty() || id.chars().any(char::is_control) {
        Err(error(
            WorkspaceErrorKind::InvalidRequest,
            "invalid lifecycle request id",
        ))
    } else {
        Ok(())
    }
}
fn context(world: &World, id: String, path: PathBuf, workspace: Entity) -> WorkspaceFlowContext {
    let installed = world.get_resource::<WorkspaceInstalled>().unwrap();
    WorkspaceFlowContext {
        id,
        workspace,
        path,
        events: world.event_sender(),
        definitions: installed.definitions.clone(),
        world: world.closure_sender(&installed.schedule),
    }
}
fn fail(
    context: &WorkspaceFlowContext,
    error: WorkspaceError,
    cleanup_completed: bool,
    restore_running: bool,
) -> FlowError {
    FlowError {
        id: context.id.clone(),
        workspace: Some(context.workspace),
        error,
        cleanup_completed,
        restore_running,
    }
}
fn members(world: &World, workspace: Entity) -> Result<BTreeSet<Entity>, WorkspaceError> {
    let mut members = world
        .get_component::<Workspace>(workspace)
        .ok_or_else(|| error(WorkspaceErrorKind::Missing, "workspace is missing"))?
        .agents
        .clone();
    for entity in world.query_with::<Agent>().result() {
        if world
            .get_component::<Agent>(entity)
            .is_some_and(|agent| agent.workspace == workspace)
        {
            members.insert(entity);
        }
    }
    Ok(members)
}
pub(crate) fn handle_start_workspace(world: &mut World, request: StartWorkspace) {
    let result = (|| {
        valid_id(&request.id)?;
        let installed = world.get_resource::<WorkspaceInstalled>().unwrap();
        let path = checked_path(installed, &request.path)?;
        if world
            .query_with::<Workspace>()
            .result()
            .into_iter()
            .any(|entity| {
                world
                    .get_component::<Workspace>(entity)
                    .is_some_and(|w| w.definition_path == path)
            })
        {
            return Err(error(
                WorkspaceErrorKind::Duplicate,
                "workspace path is already active",
            ));
        }
        let key = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("workspace path has no valid directory name"))?;
        validate_name(key)?;
        let address = Address::new(&format!("workspace:local/{key}:latest")).map_err(invalid)?;
        if world
            .query_with::<Address>()
            .result()
            .into_iter()
            .any(|entity| world.get_component::<Address>(entity) == Some(&address))
        {
            return Err(error(
                WorkspaceErrorKind::Duplicate,
                "workspace address is already active",
            ));
        }
        let workspace = world.spawn();
        world.insert_component(workspace, address);
        world.insert_component(
            workspace,
            Workspace {
                name: key.into(),
                definition_path: path.clone(),
                project_root: PathBuf::new(),
                state: WorkspaceState::Starting,
                agents: BTreeSet::new(),
            },
        );
        Ok(context(world, request.id.clone(), path, workspace))
    })();
    match result {
        Err(error) => world.send_event(StartWorkspaceResult {
            id: request.id,
            result: Err(error),
        }),
        Ok(context) => {
            let schedule = world
                .get_resource::<WorkspaceInstalled>()
                .unwrap()
                .schedule
                .clone();
            world.send_async_closure(&schedule, move || guarded_start(context));
        }
    }
}
pub(crate) fn handle_stop_workspace(world: &mut World, request: StopWorkspace) {
    let result = (|| {
        valid_id(&request.id)?;
        let component = world
            .get_component::<Workspace>(request.workspace)
            .ok_or_else(|| error(WorkspaceErrorKind::Missing, "workspace is missing"))?;
        if component.state != WorkspaceState::Running {
            return Err(error(
                WorkspaceErrorKind::StateRejected,
                "workspace is not Running",
            ));
        }
        let path = component.definition_path.clone();
        let executor = world
            .get_component::<WorkspaceExecutor>(request.workspace)
            .ok_or_else(|| {
                error(
                    WorkspaceErrorKind::CleanupFailed,
                    "workspace execution host is missing",
                )
            })?;
        executor.entry.close_admission();
        world
            .get_component_mut::<Workspace>(request.workspace)
            .unwrap()
            .state = WorkspaceState::Stopping;
        Ok(context(world, request.id.clone(), path, request.workspace))
    })();
    match result {
        Err(error) => world.send_event(StopWorkspaceResult {
            id: request.id,
            result: Err(error),
        }),
        Ok(context) => {
            let schedule = world
                .get_resource::<WorkspaceInstalled>()
                .unwrap()
                .schedule
                .clone();
            world.send_async_closure(&schedule, move || guarded_stop(context));
        }
    }
}
pub(crate) fn handle_restart_workspace(world: &mut World, request: RestartWorkspace) {
    let result = (|| {
        valid_id(&request.id)?;
        let path = checked_path(
            world.get_resource::<WorkspaceInstalled>().unwrap(),
            &request.path,
        )?;
        let matches: Vec<_> = world
            .query_with::<Workspace>()
            .result()
            .into_iter()
            .filter(|entity| {
                world
                    .get_component::<Workspace>(*entity)
                    .is_some_and(|w| w.definition_path == path)
            })
            .collect();
        let workspace = match matches.as_slice() {
            [entity] => *entity,
            [] => {
                return Err(error(
                    WorkspaceErrorKind::Missing,
                    "workspace path is not running",
                ))
            }
            _ => {
                return Err(error(
                    WorkspaceErrorKind::Duplicate,
                    "workspace path has multiple runtime entities",
                ))
            }
        };
        if world.get_component::<Workspace>(workspace).unwrap().state != WorkspaceState::Running {
            return Err(error(
                WorkspaceErrorKind::StateRejected,
                "workspace is not Running",
            ));
        }
        for member in members(world, workspace)? {
            if world
                .get_component::<ToolTasks>(member)
                .is_some_and(|tasks| {
                    tasks
                        .list()
                        .iter()
                        .any(|(_, handle)| !handle.is_completed())
                })
            {
                return Err(error(
                    WorkspaceErrorKind::Busy,
                    "workspace member has an unfinished tool task",
                ));
            }
        }
        world
            .get_component_mut::<Workspace>(workspace)
            .unwrap()
            .state = WorkspaceState::Restarting;
        Ok(context(world, request.id.clone(), path, workspace))
    })();
    match result {
        Err(error) => world.send_event(RestartWorkspaceResult {
            id: request.id,
            result: Err(error),
        }),
        Ok(context) => {
            let schedule = world
                .get_resource::<WorkspaceInstalled>()
                .unwrap()
                .schedule
                .clone();
            world.send_async_closure(&schedule, move || guarded_restart(context));
        }
    }
}
fn finish_running(
    world: &mut World,
    workspace: Entity,
    state: WorkspaceState,
) -> Result<(), WorkspaceError> {
    let component = world.get_component::<Workspace>(workspace).ok_or_else(|| {
        error(
            WorkspaceErrorKind::CleanupFailed,
            "lifecycle result has lost its workspace host",
        )
    })?;
    if component.state != state {
        return Err(error(
            WorkspaceErrorKind::StateRejected,
            "stale workspace lifecycle result",
        ));
    }
    let all = members(world, workspace)?;
    let component = world.get_component_mut::<Workspace>(workspace).unwrap();
    component.agents = all;
    component.state = WorkspaceState::Running;
    Ok(())
}
fn failure_result(world: &mut World, failure: &FlowError, state: WorkspaceState) {
    if let Some(workspace) = failure.workspace {
        if world
            .get_component::<Workspace>(workspace)
            .is_some_and(|w| w.state == state)
        {
            if failure.cleanup_completed {
                world.despawn(workspace);
            } else if failure.restore_running {
                world
                    .get_component_mut::<Workspace>(workspace)
                    .unwrap()
                    .state = WorkspaceState::Running;
            }
        }
    }
}
pub(crate) fn handle_start_result(world: &mut World, result: Result<StartOutput, StartError>) {
    match result {
        Ok(output) => {
            let result =
                finish_running(world, output.workspace, WorkspaceState::Starting).map(|()| {
                    WorkspaceOperationOutput {
                        workspace: output.workspace,
                        members: output.members,
                        member_errors: output.member_errors,
                    }
                });
            world.send_event(StartWorkspaceResult {
                id: output.id,
                result,
            });
        }
        Err(failure) => {
            failure_result(world, &failure, WorkspaceState::Starting);
            world.send_event(StartWorkspaceResult {
                id: failure.id,
                result: Err(failure.error),
            });
        }
    }
}
pub(crate) fn handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>) {
    match result {
        Ok(output) => {
            let result = if world
                .get_component::<Workspace>(output.workspace)
                .is_some_and(|w| w.state == WorkspaceState::Stopping)
            {
                world.despawn(output.workspace);
                Ok(WorkspaceOperationOutput {
                    workspace: output.workspace,
                    members: output.members,
                    member_errors: output.member_errors,
                })
            } else {
                Err(error(
                    WorkspaceErrorKind::CleanupFailed,
                    "stop result does not match its live workspace host",
                ))
            };
            world.send_event(StopWorkspaceResult {
                id: output.id,
                result,
            });
        }
        Err(failure) => {
            failure_result(world, &failure, WorkspaceState::Stopping);
            world.send_event(StopWorkspaceResult {
                id: failure.id,
                result: Err(failure.error),
            });
        }
    }
}
pub(crate) fn handle_restart_result(
    world: &mut World,
    result: Result<RestartOutput, RestartError>,
) {
    match result {
        Ok(output) => {
            let result =
                finish_running(world, output.workspace, WorkspaceState::Restarting).map(|()| {
                    WorkspaceOperationOutput {
                        workspace: output.workspace,
                        members: output.members,
                        member_errors: output.member_errors,
                    }
                });
            world.send_event(RestartWorkspaceResult {
                id: output.id,
                result,
            });
        }
        Err(failure) => {
            failure_result(world, &failure, WorkspaceState::Restarting);
            world.send_event(RestartWorkspaceResult {
                id: failure.id,
                result: Err(failure.error),
            });
        }
    }
}
async fn with_world<T: Send + 'static>(
    context: &WorkspaceFlowContext,
    operation: impl FnOnce(&mut World) -> Result<T, WorkspaceError> + Send + 'static,
) -> Result<T, WorkspaceError> {
    let (sender, receiver) = oneshot::channel();
    context.world.send(move |world| {
        let _ = sender.send(operation(world));
    });
    receiver.await.map_err(|_| {
        error(
            WorkspaceErrorKind::CleanupFailed,
            "workspace World closure did not complete",
        )
    })?
}
fn prepare_definition(
    path: &Path,
) -> Result<(WorkspaceDefinition, Vec<AgentStartParameters>), WorkspaceError> {
    reject_symlinks(path)?;
    let definition = load_workspace_definition(path)?;
    validate_definition_names(&definition)?;
    let base = path
        .parent()
        .ok_or_else(|| invalid("definition has no workspace home"))?;
    let mut parameters = Vec::new();
    for member in &definition.members {
        let directory = base.join("agents").join(&member.name);
        reject_symlinks(&directory)?;
        std::fs::create_dir_all(&directory).map_err(invalid)?;
        reject_symlinks(&directory)?;
        let database_path = directory.join("state.sqlite");
        let settings_path = directory.join("settings.toml");
        for path in [&database_path, &settings_path] {
            reject_symlinks(path)?;
            if path.exists() && !path.is_file() {
                return Err(invalid(
                    "member database and settings must be regular files",
                ));
            }
        }
        parameters.push(AgentStartParameters {
            driver: member.driver.clone(),
            project_root: definition.project_root.clone(),
            home_root: member.home_root.clone(),
            database_path,
            settings_path,
        });
    }
    Ok((definition, parameters))
}
async fn fresh_definition(
    context: &WorkspaceFlowContext,
) -> Result<(WorkspaceDefinition, Vec<AgentStartParameters>), WorkspaceError> {
    let path = context.path.clone();
    let parsed = tokio::task::spawn_blocking(move || prepare_definition(&path))
        .await
        .map_err(|e| error(WorkspaceErrorKind::InvalidDefinition, e))??;
    context
        .definitions
        .entries
        .write()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(context.path.clone(), parsed.0.clone());
    Ok(parsed)
}
async fn mount_executor(
    context: &WorkspaceFlowContext,
    definition: &WorkspaceDefinition,
    state: WorkspaceState,
) -> Result<(), WorkspaceError> {
    let executor = tokio::task::spawn_blocking(spawn_workspace_executor)
        .await
        .map_err(|e| error(WorkspaceErrorKind::ThreadStartFailed, e))??;
    let close = executor.close.clone();
    let workspace = context.workspace;
    let definition = definition.clone();
    let result = with_world(context, move |world| {
        let component = world
            .get_component::<Workspace>(workspace)
            .ok_or_else(|| error(WorkspaceErrorKind::Missing, "workspace host is missing"))?;
        if component.state != state {
            return Err(error(
                WorkspaceErrorKind::StateRejected,
                "workspace state changed before executor mount",
            ));
        }
        if world
            .get_component::<WorkspaceExecutor>(workspace)
            .is_some_and(|existing| !existing.close.is_joined())
        {
            return Err(error(
                WorkspaceErrorKind::CleanupFailed,
                "existing workspace thread has not joined",
            ));
        }
        let component = world.get_component_mut::<Workspace>(workspace).unwrap();
        component.name = definition.name;
        component.project_root = definition.project_root;
        component.agents.clear();
        world.insert_component(workspace, executor);
        Ok(())
    })
    .await;
    if let Err(error) = result {
        close.close_and_join().await.map_err(|cleanup| {
            WorkspaceError::new(
                WorkspaceErrorKind::CleanupFailed,
                format!("{error}; new executor cleanup failed: {cleanup}"),
            )
        })?;
        return Err(error);
    }
    Ok(())
}
async fn start_members(
    context: &WorkspaceFlowContext,
    definition: WorkspaceDefinition,
    parameters: Vec<AgentStartParameters>,
) -> (Vec<Entity>, Vec<MemberDiagnostic>) {
    let mut successful = Vec::new();
    let mut failures = Vec::new();
    for (member, parameters) in definition.members.into_iter().zip(parameters) {
        let (sender, receiver) = oneshot::channel();
        context.events.send_event(StartAgent {
            id: format!("{}:start:{}", context.id, member.name),
            workspace: context.workspace,
            name: member.name.clone(),
            parameters,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        let result = match receiver.await {
            Ok(Ok(agent)) => {
                let workspace = context.workspace;
                let name = member.name.clone();
                with_world(context, move |world| {
                    if !world
                        .get_component::<Agent>(agent)
                        .is_some_and(|a| a.workspace == workspace && a.name == name)
                    {
                        return Err(error(
                            WorkspaceErrorKind::MemberFailed,
                            "StartAgent replied with an unrelated or missing member entity",
                        ));
                    }
                    world.workspace_insert_agent(workspace, agent)?;
                    Ok(agent)
                })
                .await
            }
            Ok(Err(error)) => Err(WorkspaceError::new(WorkspaceErrorKind::MemberFailed, error)),
            Err(error) => Err(WorkspaceError::new(
                WorkspaceErrorKind::CleanupFailed,
                format!("member start reply closed without cleanup proof: {error}"),
            )),
        };
        match result {
            Ok(agent) => successful.push(agent),
            Err(error) => failures.push(MemberDiagnostic {
                name: member.name,
                error,
            }),
        }
    }
    (successful, failures)
}
async fn stop_members_and_join(
    context: &WorkspaceFlowContext,
    state: WorkspaceState,
) -> Result<Vec<Entity>, WorkspaceError> {
    let workspace = context.workspace;
    let (members, close) = with_world(context, move |world| {
        let component = world
            .get_component::<Workspace>(workspace)
            .ok_or_else(|| error(WorkspaceErrorKind::Missing, "workspace host is missing"))?;
        if component.state != state {
            return Err(error(
                WorkspaceErrorKind::StateRejected,
                "workspace state changed before stop",
            ));
        }
        let members = members(world, workspace)?;
        for member in &members {
            let agent = world.get_component::<Agent>(*member).ok_or_else(|| {
                error(
                    WorkspaceErrorKind::CleanupFailed,
                    "workspace contains a missing or unconfirmed member",
                )
            })?;
            if !matches!(agent.state, AgentState::Idle | AgentState::Working) {
                return Err(error(
                    WorkspaceErrorKind::Busy,
                    "member creation or lifecycle cleanup has not confirmed completion",
                ));
            }
        }
        let executor = world
            .get_component::<WorkspaceExecutor>(workspace)
            .ok_or_else(|| {
                error(
                    WorkspaceErrorKind::CleanupFailed,
                    "workspace executor is missing",
                )
            })?;
        executor.entry.close_admission();
        Ok((
            members.into_iter().collect::<Vec<_>>(),
            executor.close.clone(),
        ))
    })
    .await?;
    let mut failures = Vec::new();
    for member in &members {
        let (sender, receiver) = oneshot::channel();
        context.events.send_event(StopAgent {
            id: format!(
                "{}:stop:{}:{}",
                context.id,
                member.index(),
                member.generation()
            ),
            agent: *member,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        match receiver.await {
            Ok(Ok(stopped)) if stopped == *member => {}
            Ok(Ok(_)) => failures.push("StopAgent replied with a different Entity".to_owned()),
            Ok(Err(error)) => failures.push(error.to_string()),
            Err(error) => failures.push(format!(
                "member stop reply closed without cleanup proof: {error}"
            )),
        }
    }
    if !failures.is_empty() {
        return Err(error(
            WorkspaceErrorKind::CleanupFailed,
            failures.join("; "),
        ));
    }
    close.close_and_join().await?;
    Ok(members)
}
async fn start_flow(context: WorkspaceFlowContext) -> Result<StartOutput, StartError> {
    let (definition, parameters) = fresh_definition(&context)
        .await
        .map_err(|e| fail(&context, e, true, false))?;
    mount_executor(&context, &definition, WorkspaceState::Starting)
        .await
        .map_err(|e| fail(&context, e, false, false))?;
    let (members, member_errors) = start_members(&context, definition, parameters).await;
    Ok(StartOutput {
        id: context.id,
        workspace: context.workspace,
        members,
        member_errors,
    })
}
async fn stop_flow(context: WorkspaceFlowContext) -> Result<StopOutput, StopError> {
    let members = stop_members_and_join(&context, WorkspaceState::Stopping)
        .await
        .map_err(|e| fail(&context, e, false, false))?;
    Ok(StopOutput {
        id: context.id,
        workspace: context.workspace,
        members,
        member_errors: Vec::new(),
    })
}
async fn restart_flow(context: WorkspaceFlowContext) -> Result<RestartOutput, RestartError> {
    let (definition, parameters) = fresh_definition(&context)
        .await
        .map_err(|e| fail(&context, e, false, true))?;
    stop_members_and_join(&context, WorkspaceState::Restarting)
        .await
        .map_err(|e| fail(&context, e, false, false))?;
    mount_executor(&context, &definition, WorkspaceState::Restarting)
        .await
        .map_err(|e| fail(&context, e, false, false))?;
    let (members, member_errors) = start_members(&context, definition, parameters).await;
    Ok(RestartOutput {
        id: context.id,
        workspace: context.workspace,
        members,
        member_errors,
    })
}
async fn guarded_start(context: WorkspaceFlowContext) -> Result<StartOutput, StartError> {
    let retained = context.clone();
    AssertUnwindSafe(start_flow(context))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(fail(
                &retained,
                error(
                    WorkspaceErrorKind::CleanupFailed,
                    "workspace start flow panicked; cleanup is not confirmed",
                ),
                false,
                false,
            ))
        })
}
async fn guarded_stop(context: WorkspaceFlowContext) -> Result<StopOutput, StopError> {
    let retained = context.clone();
    AssertUnwindSafe(stop_flow(context))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(fail(
                &retained,
                error(
                    WorkspaceErrorKind::CleanupFailed,
                    "workspace stop flow panicked; cleanup is not confirmed",
                ),
                false,
                false,
            ))
        })
}
async fn guarded_restart(context: WorkspaceFlowContext) -> Result<RestartOutput, RestartError> {
    let retained = context.clone();
    AssertUnwindSafe(restart_flow(context))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(fail(
                &retained,
                error(
                    WorkspaceErrorKind::CleanupFailed,
                    "workspace restart flow panicked; cleanup is not confirmed",
                ),
                false,
                false,
            ))
        })
}
