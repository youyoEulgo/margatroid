use crate::types::WorkspaceCleanupResult;
use address::{Address, WorldAddressExt};
use agent_core::Agent;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{Entity, World};
use std::path::{Path, PathBuf};
use tool_core::{ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecuteResponse, ToolTasks};
use workspace_core::*;
fn error(kind: WorkspaceErrorKind, e: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::new(kind, e)
}
pub(crate) fn reply<T>(sender: &WorkspaceReply<T>, result: Result<T, WorkspaceError>) {
    if let Some(tx) = sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = tx.send(result);
    }
}
fn notify(w: &mut World, e: Entity, k: WorkspaceUpdateKind) {
    if let Some(c) = w.get_resource::<WorkspaceUpdateContext>().cloned() {
        c.notify(w, e, k);
    }
}
fn claim_reply<T>(original: &WorkspaceReply<T>) -> Option<WorkspaceReply<T>> {
    let sender = original.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    Some(std::sync::Arc::new(std::sync::Mutex::new(Some(sender))))
}
fn runtime(w: &World) -> &AsyncRuntimeHandle {
    w.get_resource::<AsyncRuntimeHandle>()
        .expect("async runtime required")
}
fn managed_root(root: &Path) -> Result<PathBuf, WorkspaceError> {
    if !root.is_absolute() {
        return Err(error(
            WorkspaceErrorKind::InvalidRequest,
            "Workspace root must be absolute",
        ));
    }
    if root.exists() {
        root.canonicalize()
            .map_err(|e| error(WorkspaceErrorKind::InvalidRequest, e))
    } else {
        std::fs::create_dir_all(root).map_err(|e| error(WorkspaceErrorKind::DatabaseFailed, e))?;
        root.canonicalize()
            .map_err(|e| error(WorkspaceErrorKind::DatabaseFailed, e))
    }
}
fn validate_node_name(name: &str) -> Result<(), WorkspaceError> {
    if name.starts_with("~temporary-") || name.is_empty() || name.contains('/') {
        return Err(error(
            WorkspaceErrorKind::InvalidRequest,
            "invalid ordinary node_name",
        ));
    }
    Address::new(&format!("/{name}"))
        .map(|_| ())
        .map_err(|e| error(WorkspaceErrorKind::InvalidRequest, e))
}
fn workspace_directory(root: &Path, address: &str) -> Result<PathBuf, WorkspaceError> {
    Address::new(address).map_err(|e| error(WorkspaceErrorKind::InvalidRequest, e))?;
    let name = address
        .strip_prefix('/')
        .filter(|x| !x.is_empty() && !x.contains('/'))
        .ok_or_else(|| {
            error(
                WorkspaceErrorKind::InvalidRequest,
                "Workspace requires a single root node",
            )
        })?;
    Ok(root.join(name))
}
pub(crate) fn create_workspace(w: &mut World, mut r: CreateWorkspace) {
    let Some(sender) = claim_reply(&r.reply) else {
        return;
    };
    r.reply = sender;
    let root = w.get_resource::<WorkspaceConfig>().unwrap().root.clone();
    let events = w.event_sender();
    let work = w
        .get_resource::<WorkspacePendingWork>()
        .unwrap()
        .begin_create();
    runtime(w).spawn(async move{
  let request=r.clone();let result=tokio::task::spawn_blocking(move||{
   let root=managed_root(&root)?;let directory=workspace_directory(&root,&request.address)?;
   let project=request.project_root.canonicalize().map_err(|e|error(WorkspaceErrorKind::InvalidRequest,e))?;
   if !project.is_dir()||project.starts_with(&directory)||directory.starts_with(&project){return Err(error(WorkspaceErrorKind::InvalidRequest,"project and managed directories must be separate"));}
   std::fs::create_dir(&directory).map_err(|e|error(WorkspaceErrorKind::DatabaseFailed,e))?;
   let build=(||{for category in ["normal","temporary","archived"]{std::fs::create_dir_all(directory.join("agents").join(category)).map_err(|e|error(WorkspaceErrorKind::DatabaseFailed,e))?;}
    let metadata=WorkspaceMetadata{address:request.address.clone(),alias:request.alias.clone(),project_root:project.clone()};
    let database=WorkspaceDatabase::create(directory.join("workspace.sqlite"),metadata)?;Ok(StartWorkspace{address:request.address,alias:request.alias,project_root:project,database,reply:Some(request.reply)})})();
   if build.is_err(){let verified=root.join(directory.file_name().unwrap());if verified==directory{if let Err(e)=std::fs::remove_dir_all(&directory){tracing::error!(path=%directory.display(),error=%e,"failed to reclaim new Workspace directory");}}}
   build
  }).await;
  match result{Ok(Ok(start))=>events.send_event(start),Ok(Err(e))=>reply(&r.reply,Err(e)),Err(e)=>reply(&r.reply,Err(error(WorkspaceErrorKind::DatabaseFailed,e)))}
   events.send_event(crate::types::WorkspaceWorkCompleted::new(work));
 });
}
pub(crate) fn load_workspaces(w: &mut World) {
    let root = w.get_resource::<WorkspaceConfig>().unwrap().root.clone();
    let events = w.event_sender();
    let Some(work) = w
        .get_resource::<WorkspacePendingWork>()
        .unwrap()
        .begin_load()
    else {
        return;
    };
    runtime(w).spawn(async move{
  let result=tokio::task::spawn_blocking(move||->Result<Vec<StartWorkspace>,WorkspaceError>{
   if !root.exists(){return Ok(Vec::new());}
   let root=root.canonicalize().map_err(|e|error(WorkspaceErrorKind::DatabaseFailed,e))?;let mut requests=Vec::new();
   for entry in std::fs::read_dir(&root).map_err(|e|error(WorkspaceErrorKind::DatabaseFailed,e))?{
    let entry=match entry{Ok(v)=>v,Err(e)=>{tracing::error!(error=%e,"Workspace directory entry failed");continue;}};
    let open=(||{if !entry.file_type().map_err(|e|error(WorkspaceErrorKind::DatabaseFailed,e))?.is_dir(){return Ok(None);}
     let path=entry.path();let database=WorkspaceDatabase::open(path.join("workspace.sqlite"))?;let metadata=database.metadata()?;
     if workspace_directory(&root,&metadata.address)?!=path {return Err(error(WorkspaceErrorKind::InvalidRequest,"Workspace directory does not match node"));}
     Ok(Some(StartWorkspace{address:metadata.address,alias:metadata.alias,project_root:metadata.project_root,database,reply:None}))})();
    match open{Ok(Some(r))=>requests.push(r),Ok(None)=>{},Err(e)=>tracing::error!(path=%entry.path().display(),error=%e,"Workspace load failed")}
   }Ok(requests)
  }).await;
  match result{Ok(Ok(req))=>for r in req{events.send_event(r);},Ok(Err(e))=>tracing::error!(error=%e,"Workspace scan failed"),Err(e)=>tracing::error!(error=%e,"Workspace scan worker failed")}
   events.send_event(crate::types::WorkspaceWorkCompleted::new(work));
 });
}
pub(crate) fn start_workspace(w: &mut World, mut r: StartWorkspace) {
    if let Some(original) = &r.reply {
        let Some(sender) = claim_reply(original) else {
            return;
        };
        r.reply = Some(sender);
    }
    let shared_in_world = w.query_with::<Workspace>().result().iter().any(|e| {
        w.get_component::<Workspace>(*e)
            .is_some_and(|workspace| workspace.database.same_connection(&r.database))
    });
    let result = (|| {
        let metadata = r.database.metadata()?;
        if metadata.address != r.address
            || metadata.alias != r.alias
            || metadata.project_root != r.project_root
            || !r.database.is_open()
        {
            return Err(error(
                WorkspaceErrorKind::InvalidRequest,
                "start materials do not match open database",
            ));
        }
        let root = w
            .get_resource::<WorkspaceConfig>()
            .unwrap()
            .root
            .canonicalize()
            .map_err(|e| error(WorkspaceErrorKind::InvalidRequest, e))?;
        if r.database.managed_directory() != workspace_directory(&root, &r.address)? {
            return Err(error(
                WorkspaceErrorKind::InvalidRequest,
                "database outside managed Workspace directory",
            ));
        }
        let address =
            Address::new(&r.address).map_err(|e| error(WorkspaceErrorKind::InvalidRequest, e))?;
        match w.entity_by_address(&r.address) {
            Ok(entity) => {
                let existing = w
                    .get_component::<Workspace>(entity)
                    .ok_or_else(|| error(WorkspaceErrorKind::DuplicateNode, "node occupied"))?;
                if existing.database.path() != r.database.path()
                    || existing.database.metadata()?.alias != r.alias
                    || existing.project_root != r.project_root
                {
                    return Err(error(
                        WorkspaceErrorKind::DuplicateNode,
                        "Workspace metadata mismatch",
                    ));
                }
                if !existing.database.same_connection(&r.database) {
                    r.database.close()?;
                }
                return Ok(entity);
            }
            Err(address::AddressError::EntityMissing) => {}
            Err(e) => return Err(error(WorkspaceErrorKind::DuplicateNode, e)),
        }
        let entity = w.spawn();
        w.insert_component(entity, address);
        w.insert_component(
            entity,
            Workspace::new(r.project_root.clone(), r.database.clone()),
        );
        w.insert_component(entity, ToolTasks::new());
        notify(w, entity, WorkspaceUpdateKind::Installed);
        Ok(entity)
    })();
    if result.is_err() && !shared_in_world {
        let _ = r.database.close();
    }
    if let Some(sender) = r.reply {
        reply(&sender, result);
    } else if let Err(e) = result {
        tracing::error!(error=%e,"Workspace entity start failed");
    }
}
pub(crate) fn start_agent(w: &mut World, r: StartWorkspaceAgent<ToolExecuteResponse, ToolError>) {
    let (entity, sender) = match &r {
        StartWorkspaceAgent::Existing {
            workspace, reply, ..
        }
        | StartWorkspaceAgent::New {
            workspace, reply, ..
        } => (*workspace, reply.clone()),
    };
    let result = (|| {
        let workspace = w
            .get_component::<Workspace>(entity)
            .ok_or_else(|| error(WorkspaceErrorKind::Missing, "Workspace missing"))?;
        if workspace.state != WorkspaceState::Running {
            return Err(error(
                WorkspaceErrorKind::Closed,
                "Workspace is not accepting starts",
            ));
        }
        let (initiator, path, args) = match r {
            StartWorkspaceAgent::Existing { node_name, .. } => {
                let rec = workspace.database.agent_by_node(&node_name)?;
                workspace.database.ensure_startable(&rec.id)?;
                if rec.category != AgentCategory::Normal {
                    return Err(error(
                        WorkspaceErrorKind::InvalidRequest,
                        "Existing requires a normal Agent",
                    ));
                }
                let parent = w
                    .entity_by_address(&rec.parent_node)
                    .map_err(|e| error(WorkspaceErrorKind::Missing, e))?;
                let mut arguments = rec.tool_arguments;
                let object = arguments.as_object_mut().ok_or_else(|| {
                    error(
                        WorkspaceErrorKind::InvalidRequest,
                        "stored Driver arguments must be object",
                    )
                })?;
                object.insert("node_name".into(), serde_json::json!(node_name));
                if let Some(alias) = rec.alias {
                    object.insert("alias".into(), serde_json::json!(alias));
                } else {
                    object.remove("alias");
                }
                (parent, rec.tool_path, arguments)
            }
            StartWorkspaceAgent::New {
                tool_path,
                arguments,
                ..
            } => {
                let object = arguments.as_object().ok_or_else(|| {
                    error(
                        WorkspaceErrorKind::InvalidRequest,
                        "Driver arguments must be object",
                    )
                })?;
                if let Some(v) = object.get("node_name").filter(|v| !v.is_null()) {
                    let name = v.as_str().ok_or_else(|| {
                        error(
                            WorkspaceErrorKind::InvalidRequest,
                            "node_name must be string",
                        )
                    })?;
                    validate_node_name(name)?;
                    if workspace
                        .database
                        .list_agents()?
                        .iter()
                        .any(|r| r.node_name.as_deref() == Some(name))
                    {
                        return Err(error(
                            WorkspaceErrorKind::DuplicateNode,
                            "node already registered",
                        ));
                    }
                }
                (entity, tool_path, arguments)
            }
        };
        if initiator != entity
            && w.get_component::<Agent>(initiator).is_none_or(|a| {
                a.workspace() != entity || a.state() == agent_core::AgentState::Closing
            })
        {
            return Err(error(
                WorkspaceErrorKind::ParentMismatch,
                "original parent unavailable",
            ));
        }
        if !path.is_absolute() {
            return Err(error(
                WorkspaceErrorKind::InvalidRequest,
                "Driver path must be absolute",
            ));
        }
        w.send_event(ToolExecuteRequest::Path {
            initiator,
            path,
            arguments: args,
            reply: sender.clone(),
        });
        Ok(())
    })();
    if let Err(e) = result {
        if let Some(tx) = sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                e.to_string(),
            )));
        }
    }
}
pub(crate) fn delete_workspace(w: &mut World, mut r: DeleteWorkspace) {
    let Some(sender) = claim_reply(&r.reply) else {
        return;
    };
    r.reply = sender;
    let root = w.get_resource::<WorkspaceConfig>().unwrap().root.clone();
    let prepare = (|| {
        let workspace = w
            .get_component::<Workspace>(r.workspace)
            .ok_or_else(|| error(WorkspaceErrorKind::Missing, "Workspace missing"))?;
        if workspace.state == WorkspaceState::Deleting {
            return Err(error(
                WorkspaceErrorKind::Busy,
                "Workspace already deleting",
            ));
        }
        let dir = workspace
            .database
            .path()
            .parent()
            .ok_or_else(|| {
                error(
                    WorkspaceErrorKind::InvalidRequest,
                    "database parent missing",
                )
            })?
            .to_path_buf();
        let root = root
            .canonicalize()
            .map_err(|e| error(WorkspaceErrorKind::InvalidRequest, e))?;
        let actual = dir
            .canonicalize()
            .map_err(|e| error(WorkspaceErrorKind::InvalidRequest, e))?;
        let project = if workspace.project_root.exists() {
            workspace
                .project_root
                .canonicalize()
                .map_err(|e| error(WorkspaceErrorKind::InvalidRequest, e))?
        } else {
            workspace.project_root.clone()
        };
        if actual != dir
            || !actual.starts_with(&root)
            || actual == root
            || project.starts_with(&actual)
            || actual.starts_with(&project)
        {
            return Err(error(
                WorkspaceErrorKind::InvalidRequest,
                "unsafe Workspace delete path",
            ));
        }
        let tasks = w
            .get_component::<ToolTasks>(r.workspace)
            .cloned()
            .ok_or_else(|| error(WorkspaceErrorKind::Missing, "Workspace tasks missing"))?;
        Ok((dir, workspace.database.clone(), tasks))
    })();
    let (directory, database, tasks) = match prepare {
        Ok(v) => v,
        Err(e) => {
            reply(&r.reply, Err(e));
            return;
        }
    };
    w.get_component_mut::<Workspace>(r.workspace)
        .unwrap()
        .set_state(WorkspaceState::Deleting);
    notify(w, r.workspace, WorkspaceUpdateKind::Changed);
    database.close_admission();
    tasks.close_admission();
    let events = w.event_sender();
    runtime(w).spawn(async move {
        let mut failure = None;
        let list = tasks.list();
        for (_, t) in &list {
            t.cancel();
        }
        for (_, t) in list {
            if let Err(e) = t.finish().await {
                failure = Some(error(WorkspaceErrorKind::CloseFailed, e));
            }
        }
        if failure.is_none() {
            database.wait_idle().await;
        }
        events.send_event(Ok::<_, WorkspaceError>(WorkspaceCleanupResult {
            workspace: r.workspace,
            directory,
            reply: r.reply,
            result: failure.map_or(Ok(()), Err),
        }));
    });
}
pub(crate) fn delete_result(w: &mut World, r: WorkspaceCleanupResult) {
    let result = (|| {
        r.result?;
        let workspace = w.get_component::<Workspace>(r.workspace).ok_or_else(|| {
            error(
                WorkspaceErrorKind::Missing,
                "Workspace changed during cleanup",
            )
        })?;
        if workspace.state != WorkspaceState::Deleting {
            return Err(error(
                WorkspaceErrorKind::Busy,
                "Workspace cleanup state changed",
            ));
        }
        let canonical = r
            .directory
            .canonicalize()
            .map_err(|e| error(WorkspaceErrorKind::DeleteFailed, e))?;
        if canonical != r.directory || workspace.database.managed_directory() != r.directory {
            return Err(error(
                WorkspaceErrorKind::DeleteFailed,
                "Workspace delete directory changed",
            ));
        }
        let database = workspace.database.clone();
        notify(w, r.workspace, WorkspaceUpdateKind::Closing);
        database.close()?;
        w.despawn(r.workspace);
        notify(w, r.workspace, WorkspaceUpdateKind::Removed);
        std::fs::remove_dir_all(&r.directory).map_err(|e| {
            let mut e =
                error(WorkspaceErrorKind::DeleteFailed, e).with_directory(r.directory.clone());
            e.entity_removed = true;
            e
        })?;
        Ok(())
    })();
    if result.is_err() {
        if let Some(ws) = w.get_component_mut::<Workspace>(r.workspace) {
            ws.set_state(WorkspaceState::Error);
            notify(w, r.workspace, WorkspaceUpdateKind::Changed);
        }
    }
    reply(&r.reply, result);
}
pub(crate) fn archive_agent(w: &mut World, mut r: ArchiveAgent) {
    let Some(sender) = claim_reply(&r.reply) else {
        return;
    };
    r.reply = sender;
    let prepare = (|| {
        let workspace = w
            .get_component::<Workspace>(r.workspace)
            .ok_or_else(|| error(WorkspaceErrorKind::Missing, "Workspace missing"))?;
        if workspace.state != WorkspaceState::Running {
            return Err(error(WorkspaceErrorKind::Closed, "Workspace not running"));
        }
        let database = workspace.database.clone();
        let operation = database.try_operation(&r.node_name)?;
        let record = database.agent_by_node(&r.node_name)?;
        let task = match w.entity_by_address(&record.address()) {
            Ok(e) => {
                let a = w.get_component::<Agent>(e).ok_or_else(|| {
                    error(
                        WorkspaceErrorKind::InvalidRequest,
                        "target node is not Agent",
                    )
                })?;
                if a.workspace() != r.workspace
                    || w.get_component::<Address>(a.parent())
                        .is_none_or(|parent| parent.as_str() != record.parent_node)
                {
                    return Err(error(
                        WorkspaceErrorKind::ParentMismatch,
                        "Agent Workspace or parent mismatch",
                    ));
                }
                if a.parent() != r.workspace
                    && w.get_component::<Agent>(a.parent())
                        .is_none_or(|parent| parent.workspace() != r.workspace)
                {
                    return Err(error(
                        WorkspaceErrorKind::ParentMismatch,
                        "Agent parent outside Workspace",
                    ));
                }
                Some(
                    w.get_component::<ToolTasks>(a.parent())
                        .ok_or_else(|| error(WorkspaceErrorKind::Missing, "parent tasks missing"))?
                        .get(a.task_id())
                        .map_err(|e| error(WorkspaceErrorKind::Missing, e))?,
                )
            }
            Err(address::AddressError::EntityMissing) => None,
            Err(e) => return Err(error(WorkspaceErrorKind::InvalidRequest, e)),
        };
        Ok((database, operation, record, task))
    })();
    let (database, operation, record, task) = match prepare {
        Ok(v) => v,
        Err(e) => {
            reply(&r.reply, Err(e));
            return;
        }
    };
    runtime(w).spawn(async move {
        if let Some(task) = task {
            if let Err(e) = task.finish().await {
                reply(&r.reply, Err(error(WorkspaceErrorKind::CloseFailed, e)));
                return;
            }
        }
        let result =
            tokio::task::spawn_blocking(move || database.archive_agent(&operation, &record.id))
                .await;
        reply(
            &r.reply,
            match result {
                Ok(v) => v.map(|_| ()),
                Err(e) => Err(error(WorkspaceErrorKind::ArchivePending, e)),
            },
        );
    });
}
