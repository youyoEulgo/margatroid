use crate::{
    types::{CloseControl, CloseStatus},
    AgentInstalled,
};
use address::{Address, WorldAddressExt};
use agent_core::{
    Agent, AgentCloseFuture, AgentError, AgentErrorKind, AgentHandle, AgentReply, AgentState,
    AgentUpdateContext, AgentUpdateKind, StartAgent, StopAgent, WorldAgentExt,
};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::{Entity, World};
use futures_util::{future::join_all, FutureExt};
use inference_core::ActiveInference;
use mcl_core::{MclQueue, WorldMclExt};
use std::{panic::AssertUnwindSafe, sync::Arc};
use storage_core::AgentStorage;
use tokio::sync::oneshot;
use tool_core::{ToolError, ToolErrorKind, ToolTaskHandle, ToolTasks};
use tool_resource_core::Tools;
use workspace_core::{Workspace, WorkspaceState};

fn error(kind: AgentErrorKind, message: impl std::fmt::Display) -> AgentError {
    AgentError::new(kind, message.to_string())
}
fn cleanup(message: impl std::fmt::Display) -> AgentError {
    error(AgentErrorKind::CleanupFailed, message)
}
fn task_close(handle: AgentHandle) -> tool_core::TaskChildClose {
    Arc::new(move || {
        let wait = handle.close();
        Box::pin(async move {
            wait.await
                .map_err(|e| ToolError::new(ToolErrorKind::CleanupFailed, e.to_string()))
        })
    })
}

pub(crate) fn handle_start_agent(world: &mut World, request: StartAgent<AgentStorage>) {
    let Some(reply) = request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    else {
        return;
    };
    let installed = world
        .get_resource::<AgentInstalled>()
        .expect("AgentPlugin missing")
        .clone();
    let prepared = (|| {
        let workspace = world
            .get_component::<Workspace>(request.workspace)
            .ok_or_else(|| error(AgentErrorKind::InvalidRequest, "Workspace missing"))?;
        if workspace.state != WorkspaceState::Running {
            return Err(error(
                AgentErrorKind::StateRejected,
                "Workspace is not running",
            ));
        }
        let same_workspace = if world.contains_component::<Workspace>(request.parent) {
            request.parent == request.workspace
        } else {
            world
                .get_component::<Agent>(request.parent)
                .is_some_and(|a| {
                    a.workspace() == request.workspace && a.state() != AgentState::Closing
                })
        };
        if !same_workspace {
            return Err(error(
                AgentErrorKind::InvalidRequest,
                "parent does not belong to Workspace",
            ));
        }
        let parent_address = world
            .get_component::<Address>(request.parent)
            .ok_or_else(|| error(AgentErrorKind::InvalidRequest, "parent Address missing"))?;
        let address =
            Address::new(&request.address).map_err(|e| error(AgentErrorKind::InvalidRequest, e))?;
        let (parent, leaf) = request.address.rsplit_once('/').ok_or_else(|| {
            error(
                AgentErrorKind::InvalidRequest,
                "Agent address requires a parent",
            )
        })?;
        if leaf.is_empty() || parent != parent_address.as_str() {
            return Err(error(
                AgentErrorKind::InvalidRequest,
                "Agent address does not match actual parent",
            ));
        }
        if world
            .address_exists(address.as_str())
            .map_err(|e| error(AgentErrorKind::InvalidRequest, e))?
        {
            return Err(error(
                AgentErrorKind::StateRejected,
                "Agent address already exists",
            ));
        }
        if !request.project_root.is_absolute() || !request.ui_path.is_absolute() {
            return Err(error(
                AgentErrorKind::InvalidRequest,
                "Agent paths must be absolute",
            ));
        }
        if !request.persistence.is_open() {
            return Err(error(
                AgentErrorKind::InvalidRequest,
                "Agent storage is not open",
            ));
        }
        let tasks = world
            .get_component::<ToolTasks>(request.parent)
            .ok_or_else(|| error(AgentErrorKind::InvalidRequest, "parent ToolTasks missing"))?;
        if !tasks.is_accepting() {
            return Err(error(
                AgentErrorKind::StateRejected,
                "parent task admission is closed",
            ));
        }
        let task = tasks
            .get(&request.task_id)
            .map_err(|e| error(AgentErrorKind::StateRejected, e))?;
        if task.cancel_token().is_cancelled() || task.is_cleanup_complete() {
            return Err(error(
                AgentErrorKind::StateRejected,
                "creator task is closing",
            ));
        }
        Ok((address, task))
    })();
    let (address, parent_task) = match prepared {
        Ok(v) => v,
        Err(e) => {
            let _ = reply.send(Err(e));
            return;
        }
    };
    let entity = world.spawn();
    let agent = Agent::new(
        request.workspace,
        request.parent,
        request.task_id,
        request.project_root,
    );
    let ready = agent.ready_receiver();
    world.insert_component(entity, address);
    world.insert_component(entity, agent);
    world.insert_component(entity, MclQueue::default());
    world.insert_component(entity, Tools::new());
    world.insert_component(entity, ToolTasks::new());
    world.insert_component(entity, ActiveInference::default());
    world.insert_component(entity, request.persistence);
    let sender = world.closure_sender(&installed.schedule);
    let control = CloseControl::new();
    let handle = AgentHandle::new(entity, ready, {
        let control = control.clone();
        move || request_close(sender.clone(), entity, control.clone())
    });
    if let Err(e) = parent_task.bind_child(entity, task_close(handle.clone())) {
        retain_and_rollback(
            world
                .get_resource::<AsyncRuntimeHandle>()
                .expect("async runtime missing"),
            parent_task,
            handle,
            Some(reply),
            error(AgentErrorKind::StartFailed, e),
        );
        return;
    }
    let publication = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut waits = installed
            .sync
            .notify(world, entity, AgentUpdateKind::Installed);
        waits.extend(installed.sync.notify(
            world,
            entity,
            AgentUpdateKind::SourcesInstalled {
                ui_path: Some(request.ui_path),
            },
        ));
        waits
    }));
    match publication {
        Ok(waits) if waits.is_empty() => {}
        Ok(waits) => {
            control
                .observations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(waits);
            retain_and_rollback(
                world
                    .get_resource::<AsyncRuntimeHandle>()
                    .expect("async runtime missing"),
                parent_task,
                handle,
                Some(reply),
                cleanup("initial source installation returned pending cleanup"),
            );
            return;
        }
        Err(_) => {
            retain_and_rollback(
                world
                    .get_resource::<AsyncRuntimeHandle>()
                    .expect("async runtime missing"),
                parent_task,
                handle,
                Some(reply),
                cleanup("Agent source installation panicked"),
            );
            return;
        }
    }
    if let Err(Ok(handle)) = reply.send(Ok(handle)) {
        retain_and_rollback(
            world
                .get_resource::<AsyncRuntimeHandle>()
                .expect("async runtime missing"),
            parent_task,
            handle,
            None,
            error(
                AgentErrorKind::ReplyClosed,
                "Agent creation receiver closed",
            ),
        );
    }
}

fn retain_and_rollback(
    runtime: &AsyncRuntimeHandle,
    parent: ToolTaskHandle,
    handle: AgentHandle,
    reply: Option<AgentReply>,
    failure: AgentError,
) {
    if let Err(e) = parent.retain_child_for_cleanup(handle.entity(), task_close(handle.clone())) {
        tracing::error!(%e,"could not retain failed Agent creation cleanup");
    }
    runtime.spawn(async move {
        let result = handle.close().await;
        let failure = match result {
            Ok(()) => failure,
            Err(e) => cleanup(format!("{failure}; creation rollback: {e}")),
        };
        if let Some(reply) = reply {
            let _ = reply.send(Err(failure));
        }
    });
}

pub(crate) fn handle_stop_agent(world: &mut World, request: StopAgent) {
    let Some(reply) = request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    else {
        return;
    };
    let task = (|| {
        let entity = world
            .entity_by_address(&request.address)
            .map_err(|e| error(AgentErrorKind::AgentMissing, e))?;
        let agent = world
            .get_component::<Agent>(entity)
            .ok_or_else(|| error(AgentErrorKind::AgentMissing, "target is not an Agent"))?;
        let parent = world
            .get_component::<Address>(agent.parent())
            .ok_or_else(|| error(AgentErrorKind::StopFailed, "actual parent missing"))?;
        if request.address.rsplit_once('/').map(|v| v.0) != Some(parent.as_str()) {
            return Err(error(
                AgentErrorKind::StopFailed,
                "Agent parent address mismatch",
            ));
        }
        world
            .get_component::<ToolTasks>(agent.parent())
            .ok_or_else(|| error(AgentErrorKind::StopFailed, "parent tasks missing"))?
            .get(agent.task_id())
            .map_err(|e| error(AgentErrorKind::StopFailed, e))
    })();
    match task {
        Err(e) => {
            let _ = reply.send(Err(e));
        }
        Ok(task) => {
            let runtime = world
                .get_resource::<AsyncRuntimeHandle>()
                .expect("async runtime missing");
            runtime.spawn(async move {
                let result = task.finish().await.map_err(cleanup);
                let _ = reply.send(result);
            });
        }
    }
}

fn request_close(
    sender: WorldClosureSender,
    entity: Entity,
    control: Arc<CloseControl>,
) -> AgentCloseFuture {
    let mut rx = control.result.subscribe();
    {
        let mut status = control.status.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*status, CloseStatus::Closed) {
            return Box::pin(async { Ok(()) });
        }
        if !matches!(*status, CloseStatus::Running) {
            *status = CloseStatus::Running;
            control.result.send_replace(CloseStatus::Running);
            let ctl = control.clone();
            let execute = sender.clone();
            sender.send(move |world| {
                world
                    .get_resource::<AsyncRuntimeHandle>()
                    .expect("async runtime missing")
                    .spawn(async move {
                        let result = AssertUnwindSafe(close_agent(execute, entity, ctl.clone()))
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| Err(cleanup("Agent close supervisor panicked")));
                        let state = match result {
                            Ok(()) => CloseStatus::Closed,
                            Err(e) => CloseStatus::Failed(e),
                        };
                        let mut phase = ctl.status.lock().unwrap_or_else(|e| e.into_inner());
                        *phase = state.clone();
                        ctl.result.send_replace(state);
                    });
            });
        }
    }
    Box::pin(async move {
        loop {
            let status = rx.borrow_and_update().clone();
            match status {
                CloseStatus::Closed => return Ok(()),
                CloseStatus::Failed(e) => return Err(e),
                _ => {}
            }
            rx.changed()
                .await
                .map_err(|_| cleanup("Agent close supervisor disconnected"))?;
        }
    })
}

async fn world_call<R: Send + 'static>(
    sender: &WorldClosureSender,
    op: impl FnOnce(&mut World) -> Result<R, AgentError> + Send + 'static,
) -> Result<R, AgentError> {
    let (tx, rx) = oneshot::channel();
    sender.send(move |world| {
        let _ = tx.send(op(world));
    });
    rx.await.map_err(|_| cleanup("World close response lost"))?
}
async fn close_agent(
    sender: WorldClosureSender,
    entity: Entity,
    control: Arc<CloseControl>,
) -> Result<(), AgentError> {
    let context = world_call(&sender, move |world| {
        if !world.is_alive(entity) {
            return Ok(None);
        }
        if !world.contains_component::<Agent>(entity) {
            return Err(cleanup("close target is no longer an Agent"));
        }
        world.agent_begin_close(entity)?;
        world.mcl_close(entity).map_err(cleanup)?;
        let drain = world.mcl_drain(entity).map_err(cleanup)?;
        let tasks = world
            .get_component::<ToolTasks>(entity)
            .ok_or_else(|| cleanup("Agent tasks missing"))?
            .clone();
        tasks.close_admission();
        for (_, task) in tasks.list() {
            task.cancel();
        }
        let inference = world
            .get_component::<ActiveInference>(entity)
            .and_then(|v| v.current.clone());
        if let Some(handle) = &inference {
            handle.retire_source();
            handle.cancel();
        }
        let storage = world
            .get_component::<AgentStorage>(entity)
            .ok_or_else(|| cleanup("Agent storage missing"))?
            .clone();
        Ok(Some((drain, tasks, inference, storage)))
    })
    .await?;
    let Some((drain, tasks, inference, storage)) = context else {
        return Ok(());
    };
    let mut errors = Vec::new();
    if let Err(e) = drain.wait().await {
        errors.push(e.to_string());
    }
    if let Some(handle) = inference {
        if let Err(e) = handle.wait_finished().await {
            errors.push(e.to_string());
        }
    }
    let results = join_all(
        tasks
            .list()
            .into_iter()
            .map(|(_, task)| async move { task.finish().await }),
    )
    .await;
    for result in results {
        if let Err(e) = result {
            errors.push(e.to_string());
        }
    }
    if !errors.is_empty() {
        return Err(cleanup(errors.join("; ")));
    }
    let waits = world_call(&sender, move |world| {
        let sync = world
            .get_resource::<AgentUpdateContext>()
            .cloned()
            .ok_or_else(|| cleanup("Agent update context missing"))?;
        Ok(sync.notify(world, entity, AgentUpdateKind::SourcesClosing))
    })
    .await?;
    control
        .observations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(waits);
    let waits = std::mem::take(
        &mut *control
            .observations
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
    );
    for result in join_all(waits).await {
        if let Err(e) = result {
            errors.push(e.to_string());
        }
    }
    if !errors.is_empty() {
        return Err(cleanup(errors.join("; ")));
    }
    tokio::task::spawn_blocking(move || storage_core::close_storage(&storage))
        .await
        .map_err(cleanup)?
        .map_err(cleanup)?;
    world_call(&sender, move |world| {
        if world.is_alive(entity) {
            agent_core::remove_agent(world, entity);
        }
        Ok(())
    })
    .await
}
