use workspace_core::{remove_workspace, update_workspace, WorkspaceChange};
fn reply(
    sender: &workspace_core::WorkspaceOperationReply,
    result: Result<workspace_core::WorkspaceOperationOutput, workspace_core::WorkspaceError>,
) {
    if let Some(sender) = sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = sender.send(result);
    }
}
use crate::{checked_path, invalid, reject_symlinks, types::*, validate_name, WorkspaceInstalled};
use address::Address;
use agent_core::{Agent, AgentState, StopAgent};
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
fn context(
    world: &World,
    id: String,
    path: PathBuf,
    workspace: Entity,
    reply: WorkspaceOperationReply,
) -> WorkspaceFlowContext {
    let installed = world.get_resource::<WorkspaceInstalled>().unwrap();
    WorkspaceFlowContext {
        state: world.get_component::<Workspace>(workspace).unwrap().state,
        startup: world
            .get_component::<StartupBarrier>(workspace)
            .expect("Workspace startup barrier missing")
            .0
            .clone(),
        id,
        reply,
        workspace,
        path,
        events: world.event_sender(),
        definitions: installed.definitions.clone(),
        program_root: installed.program_root.clone(),
        world: world.closure_sender(&installed.schedule),
    }
}
fn fail(
    context: &WorkspaceFlowContext,
    error: WorkspaceError,
    cleanup_completed: bool,
) -> FlowError {
    FlowError {
        reply: context.reply.clone(),
        workspace: Some(context.workspace),
        error,
        cleanup_completed,
        state: Some(context.state),
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
            .is_some_and(|agent| agent.workspace() == workspace)
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
        world.insert_component(
            workspace,
            StartupBarrier(Arc::new(StartupCompletion::default())),
        );
        world.insert_component(workspace, address);
        world.insert_component(
            workspace,
            workspace_core::Workspace::new(
                key.into(),
                path.clone(),
                PathBuf::new(),
                WorkspaceState::Starting,
                BTreeSet::new(),
                None,
                world
                    .get_resource::<workspace_core::WorkspaceUpdateContext>()
                    .expect("workspace sync context missing")
                    .clone(),
            ),
        );
        world
            .get_component::<Workspace>(workspace)
            .unwrap()
            .updates()
            .notify(world, workspace);
        Ok(context(
            world,
            request.id.clone(),
            path,
            workspace,
            request.reply.clone(),
        ))
    })();
    match result {
        Err(error) => reply(&request.reply, Err(error)),
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
        if component.state == WorkspaceState::Stopping {
            return Err(error(
                WorkspaceErrorKind::StateRejected,
                "workspace is already stopping",
            ));
        }
        let path = component.definition_path.clone();
        if let Some(executor) = world.get_component::<WorkspaceExecutor>(request.workspace) {
            executor.entry.close_admission();
        }
        // A Starting Workspace may still await a Lua startup handoff. Request
        // cancellation now, then let stop_flow await its actual startup barrier.
        for entity in world
            .query_with::<lua_runtime_core::WorkspaceLuaTask>()
            .result()
        {
            let task = world
                .get_component::<lua_runtime_core::WorkspaceLuaTask>(entity)
                .unwrap();
            if task.workspace != request.workspace {
                continue;
            }
            if let Some(vm) = world.get_component::<lua_runtime_core::DriverLuaVm>(entity) {
                vm.cancel.cancel();
            }
        }
        update_workspace(
            world,
            request.workspace,
            WorkspaceChange::State(WorkspaceState::Stopping),
        )?;
        Ok(context(
            world,
            request.id.clone(),
            path,
            request.workspace,
            request.reply.clone(),
        ))
    })();
    match result {
        Err(error) => reply(&request.reply, Err(error)),
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
        let path = checked_path(
            world.get_resource::<WorkspaceInstalled>().unwrap(),
            &request.path,
        )?;
        world
            .query_with::<Workspace>()
            .result()
            .into_iter()
            .find(|entity| {
                world
                    .get_component::<Workspace>(*entity)
                    .is_some_and(|workspace| workspace.definition_path == path)
            })
            .ok_or_else(|| error(WorkspaceErrorKind::Missing, "workspace path is not running"))
    })();
    let workspace = match result {
        Ok(workspace) => workspace,
        Err(failure) => {
            reply(&request.reply, Err(failure));
            return;
        }
    };
    let (sender, receiver) = oneshot::channel();
    handle_stop_workspace(
        world,
        StopWorkspace {
            id: request.id.clone(),
            workspace,
            reply: Arc::new(Mutex::new(Some(sender))),
        },
    );
    let events = world.event_sender();
    let schedule = world
        .get_resource::<WorkspaceInstalled>()
        .unwrap()
        .schedule
        .clone();
    world.send_async_closure(&schedule, move || async move {
        match receiver.await {
            Ok(Ok(_)) => events.send_event(StartWorkspace {
                id: request.id,
                path: request.path,
                reply: request.reply,
            }),
            Ok(Err(failure)) => reply(&request.reply, Err(failure)),
            Err(_) => reply(
                &request.reply,
                Err(error(
                    WorkspaceErrorKind::CleanupFailed,
                    "workspace stop reply was lost; restart cannot start",
                )),
            ),
        }
        Ok::<_, async_runtime_plugin::AsyncTaskError>(())
    });
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
    update_workspace(world, workspace, WorkspaceChange::Members(all))?;
    world.remove_component::<WorkspaceFailure>(workspace);
    update_workspace(world, workspace, WorkspaceChange::Failure(None))?;
    update_workspace(
        world,
        workspace,
        WorkspaceChange::State(WorkspaceState::Running),
    )?;
    Ok(())
}
fn failure_result(world: &mut World, failure: &FlowError, state: WorkspaceState) {
    if let Some(workspace) = failure.workspace {
        if world
            .get_component::<Workspace>(workspace)
            .is_some_and(|w| w.state == failure.state.unwrap_or(state))
        {
            world.insert_component(workspace, WorkspaceFailure(failure.error.clone()));
            let _ = update_workspace(
                world,
                workspace,
                WorkspaceChange::Failure(Some(failure.error.clone())),
            );
            if failure.cleanup_completed {
                remove_workspace(world, workspace);
            } else {
                let _ = update_workspace(
                    world,
                    workspace,
                    WorkspaceChange::State(WorkspaceState::Error),
                );
            }
        }
    }
}
pub(crate) fn handle_start_result(world: &mut World, result: Result<StartOutput, StartError>) {
    match result {
        Ok(output) => {
            if world
                .get_component::<Workspace>(output.workspace)
                .is_none_or(|workspace| workspace.state != WorkspaceState::Starting)
            {
                reply(
                    &output.reply,
                    Err(error(
                        WorkspaceErrorKind::StateRejected,
                        "startup superseded by cleanup",
                    )),
                );
                return;
            }
            if !output.member_errors.is_empty() {
                if let Ok(all) = members(world, output.workspace) {
                    let _ =
                        update_workspace(world, output.workspace, WorkspaceChange::Members(all));
                }
                let failure = error(
                    WorkspaceErrorKind::MemberFailed,
                    output
                        .member_errors
                        .iter()
                        .map(|item| format!("{}: {}", item.name, item.error))
                        .collect::<Vec<_>>()
                        .join("; "),
                );
                let _ = update_workspace(
                    world,
                    output.workspace,
                    WorkspaceChange::Failure(Some(failure.clone())),
                );
                let _ = update_workspace(
                    world,
                    output.workspace,
                    WorkspaceChange::State(WorkspaceState::Error),
                );
                reply(&output.reply, Err(failure));
                return;
            }
            let result =
                finish_running(world, output.workspace, WorkspaceState::Starting).map(|()| {
                    WorkspaceOperationOutput {
                        workspace: output.workspace,
                        members: output.members,
                        member_errors: output.member_errors.clone(),
                    }
                });
            reply(&output.reply, result);
        }
        Err(failure) => {
            failure_result(world, &failure, WorkspaceState::Starting);
            reply(&failure.reply, Err(failure.error));
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
                remove_workspace(world, output.workspace);
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
            reply(&output.reply, result);
        }
        Err(failure) => {
            failure_result(world, &failure, WorkspaceState::Stopping);
            reply(&failure.reply, Err(failure.error));
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
    definition: WorkspaceDefinition,
) -> Result<WorkspaceDefinition, WorkspaceError> {
    let base = path
        .parent()
        .ok_or_else(|| invalid("definition has no workspace home"))?;
    for member in &definition.members {
        let directory = base.join("agents").join(&member.name);
        reject_symlinks(&directory)?;
        std::fs::create_dir_all(&directory).map_err(invalid)?;
        reject_symlinks(&directory)?;
        let database_path = directory.join("state.sqlite");
        let settings_path = directory.join("setting.toml");
        for path in [&database_path, &settings_path] {
            reject_symlinks(path)?;
            if path.exists() && !path.is_file() {
                return Err(invalid(
                    "member database and settings must be regular files",
                ));
            }
        }
    }
    Ok(definition)
}
async fn fresh_definition(
    context: &WorkspaceFlowContext,
) -> Result<WorkspaceDefinition, WorkspaceError> {
    let path = context.path.clone();
    let definitions = context.definitions.clone();
    tokio::task::spawn_blocking(move || {
        let snapshot = definitions.reload()?;
        let definition = snapshot.get(&path).cloned().ok_or_else(|| {
            error(
                WorkspaceErrorKind::Missing,
                "workspace is absent from reloaded definitions",
            )
        })?;
        prepare_definition(&path, definition)
    })
    .await
    .map_err(|e| error(WorkspaceErrorKind::InvalidDefinition, e))?
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
        update_workspace(world, workspace, WorkspaceChange::Definition(definition))?;
        update_workspace(world, workspace, WorkspaceChange::Members(BTreeSet::new()))?;
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
) -> (Vec<Entity>, Vec<MemberDiagnostic>) {
    let mut failures = Vec::new();
    let mut replies = Vec::new();
    for member in definition.members.into_iter() {
        let workspace = context.workspace;
        let name = member.name.clone();
        let driver = member.driver.clone();
        let (sender, receiver) = oneshot::channel();
        let root = context.program_root.clone();
        let driver = driver.clone();
        let source = tokio::task::spawn_blocking(move || {
            let path = address::resolve_file(&root, &driver).map_err(invalid)?;
            agent_driver_core::read_driver(&path, &Default::default()).map_err(invalid)
        })
        .await
        .map_err(invalid)
        .and_then(|value| value);
        let source = match source {
            Ok(value) => value,
            Err(error) => {
                failures.push(MemberDiagnostic {
                    name: member.name,
                    error,
                });
                continue;
            }
        };
        let prepared=with_world(context,move|world|{
            let component=world.get_component::<Workspace>(workspace).ok_or_else(||error(WorkspaceErrorKind::Missing,"Workspace missing"))?;
            let path=source.origin;let program=source.program;
            let workspace_address=world.get_component::<Address>(workspace).unwrap().as_str().to_owned();
            let execution_address=Address::new(&format!("execution:{}/{}:latest",workspace.index(),name)).map_err(invalid)?;
            let environment=lua_runtime_core::LuaEnvironment{globals:std::collections::BTreeMap::from([("execution_context".into(),lua_runtime_core::LuaBinding::Value(lua_runtime_core::LuaValue::from_json(serde_json::json!({"workspace":{"address":workspace_address,"name":component.name,"project_root":component.project_root},"name":name,"resource_dir":path.parent().unwrap()})).map_err(invalid)?))]),modules:Default::default()};
            let owner=world.spawn();world.insert_component(owner,execution_address);world.insert_component(owner,lua_runtime_core::WorkspaceLuaTask{workspace,name});
            world.send_event(lua_runtime_core::StartDriverLuaVm{owner,workspace,program:lua_runtime_core::LuaProgram{source:program,origin:path,},environment,reply:Arc::new(Mutex::new(Some(sender)))});Ok(owner)
        }).await;
        match prepared {
            Ok(owner) => replies.push((member, owner, receiver)),
            Err(error) => failures.push(MemberDiagnostic {
                name: member.name,
                error,
            }),
        }
    }
    let outcomes = futures_util::future::join_all(
        replies
            .into_iter()
            .map(|(member, owner, receiver)| async move { (member, owner, receiver.await) }),
    )
    .await;
    for (member, owner, received) in outcomes {
        match received {
            Ok(Ok(instance)) if instance.owner == owner => {}
            other => failures.push(MemberDiagnostic {
                name: member.name,
                error: error(
                    WorkspaceErrorKind::MemberFailed,
                    format!("Lua task start failed: {other:?}"),
                ),
            }),
        }
    }
    let workspace = context.workspace;
    let agents = with_world(context, move |world| {
        Ok(members(world, workspace)?.into_iter().collect())
    })
    .await;
    match agents {
        Ok(agents) => (agents, failures),
        Err(error) => {
            failures.push(MemberDiagnostic {
                name: "Workspace".into(),
                error,
            });
            (Vec::new(), failures)
        }
    }
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
        let members = members(world, workspace)?
            .into_iter()
            .filter(|entity| {
                world
                    .get_component::<agent_core::AgentOwner>(*entity)
                    .is_none_or(|owner| owner.0 == workspace)
            })
            .collect::<BTreeSet<_>>();
        for member in &members {
            let agent = world.get_component::<Agent>(*member).ok_or_else(|| {
                error(
                    WorkspaceErrorKind::CleanupFailed,
                    "workspace contains a missing or unconfirmed member",
                )
            })?;
            if agent.state() == AgentState::Stopping {
                return Err(error(
                    WorkspaceErrorKind::Busy,
                    "member creation or lifecycle cleanup has not confirmed completion",
                ));
            }
        }
        let close = world
            .get_component::<WorkspaceExecutor>(workspace)
            .map(|executor| {
                executor.entry.close_admission();
                executor.close.clone()
            });
        Ok((members.into_iter().collect::<Vec<_>>(), close))
    })
    .await?;
    let executions = with_world(context, move |world| {
        Ok(world
            .query_with::<lua_runtime_core::WorkspaceLuaTask>()
            .result()
            .into_iter()
            .filter_map(|entity| {
                let task = world.get_component::<lua_runtime_core::WorkspaceLuaTask>(entity)?;
                if task.workspace != workspace {
                    return None;
                }
                Some((
                    entity,
                    world
                        .get_component::<lua_runtime_core::DriverLuaVm>(entity)
                        .map(|vm| vm.instance.clone()),
                ))
            })
            .collect::<Vec<_>>())
    })
    .await?;
    for (entity, instance) in executions {
        if let Some(instance) = instance {
            let (tx, rx) = oneshot::channel();
            context
                .events
                .send_event(lua_runtime_core::StopDriverLuaVm {
                    instance,
                    reply: Arc::new(Mutex::new(Some(tx))),
                });
            rx.await
                .map_err(|e| error(WorkspaceErrorKind::CleanupFailed, e))?
                .map_err(|e| error(WorkspaceErrorKind::CleanupFailed, e))?;
        }
        with_world(context, move |world| {
            world.despawn(entity);
            Ok(())
        })
        .await?;
    }
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
    with_world(context, move |world| {
        update_workspace(world, workspace, WorkspaceChange::Members(BTreeSet::new()))
    })
    .await?;
    if let Some(close) = close {
        close.close_and_join().await?;
    }
    Ok(members)
}
async fn start_flow(context: WorkspaceFlowContext) -> Result<StartOutput, StartError> {
    let definition = fresh_definition(&context)
        .await
        .map_err(|e| fail(&context, e, true))?;
    mount_executor(&context, &definition, WorkspaceState::Starting)
        .await
        .map_err(|e| fail(&context, e, false))?;
    let (members, member_errors) = start_members(&context, definition).await;
    Ok(StartOutput {
        reply: context.reply,
        workspace: context.workspace,
        members,
        member_errors,
    })
}
async fn stop_flow(context: WorkspaceFlowContext) -> Result<StopOutput, StopError> {
    context.startup.wait().await;
    let members = stop_members_and_join(&context, WorkspaceState::Stopping)
        .await
        .map_err(|e| fail(&context, e, false))?;
    Ok(StopOutput {
        reply: context.reply,
        workspace: context.workspace,
        members,
        member_errors: Vec::new(),
    })
}
async fn guarded_start(context: WorkspaceFlowContext) -> Result<StartOutput, StartError> {
    let retained = context.clone();
    let result = AssertUnwindSafe(start_flow(context))
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
            ))
        });
    retained.startup.finish();
    result
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
            ))
        })
}
