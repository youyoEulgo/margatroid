use crate::probes::{state_source, update};
use client_core::{ClientFrame, ClientPrepareError, Clients, SyncContext, SyncKey};
use core_plugin::{Entity, World};
use margatroid_protocol::*;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use workspace_core::{Workspace, WorkspaceUpdateContext, WorkspaceUpdateKind};
struct WorkspaceViews {
    rows: Mutex<BTreeMap<Entity, WorkspaceView>>,
    view: Arc<Mutex<(u64, Vec<WorkspaceInfoDto>)>>,
    sync: SyncContext<Vec<WorkspaceInfoDto>>,
    sources: Mutex<BTreeMap<Entity, WorkspaceWatch>>,
}
struct WorkspaceView {
    database: workspace_core::WorkspaceDatabase,
    revision: u64,
    row: WorkspaceInfoDto,
}
impl WorkspaceView {
    fn project(
        &mut self,
        database: workspace_core::WorkspaceDatabase,
        mut row: WorkspaceInfoDto,
        revision: u64,
    ) {
        let revision = if self.database.same_connection(&database) && self.revision > revision {
            row.alias = self.row.alias.clone();
            row.agents = self.row.agents.clone();
            self.revision
        } else {
            revision
        };
        *self = Self {
            database,
            row,
            revision,
        };
    }
    fn apply_snapshot(
        &mut self,
        database: &workspace_core::WorkspaceDatabase,
        snapshot: workspace_core::WorkspaceRecordsSnapshot,
    ) {
        if self.database.same_connection(database) && snapshot.revision >= self.revision {
            self.row.alias = snapshot.metadata.alias;
            self.row.agents = snapshot
                .agents
                .into_iter()
                .map(crate::projection::workspace_agent)
                .collect();
            self.revision = snapshot.revision;
        }
    }
}
struct WorkspaceWatch {
    database: workspace_core::WorkspaceDatabase,
    active: Arc<AtomicBool>,
    cancel: tokio_util::sync::CancellationToken,
}
impl Drop for WorkspaceWatch {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        self.cancel.cancel();
    }
}
impl Drop for WorkspaceViews {
    fn drop(&mut self) {
        self.sync.close();
    }
}
impl WorkspaceViews {
    fn publish(&self) {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let mut values: Vec<_> = rows.values().map(|value| value.row.clone()).collect();
        values.sort_by(|a, b| a.address.cmp(&b.address));
        update(&self.view, &self.sync, values);
    }
}
#[derive(Clone)]
pub struct WorkspaceSyncBinding {
    pub updates: WorkspaceUpdateContext,
    views: Arc<WorkspaceViews>,
}
impl core_plugin::Resource for WorkspaceSyncBinding {}
pub fn workspace_sync_binding(clients: Clients) -> WorkspaceSyncBinding {
    let view = Arc::new(Mutex::new((0, Vec::<WorkspaceInfoDto>::new())));
    let sync = state_source(
        clients,
        crate::sync_selectors(),
        SyncKey {
            topic: "workspace.sync".into(),
            object: "all".into(),
        },
        &view,
        |workspaces| {
            encode_server_message(&ServerMessage::WorkspaceSync { workspaces })
                .map(|s| Arc::new(ClientFrame::Text(s)))
                .map_err(|_| ClientPrepareError::TooLarge)
        },
    );
    let views = Arc::new(WorkspaceViews {
        rows: Mutex::default(),
        view,
        sync,
        sources: Mutex::default(),
    });
    let source = views.clone();
    let updates = WorkspaceUpdateContext::new(move |world, owner, kind| {
        if matches!(
            kind,
            WorkspaceUpdateKind::Closing | WorkspaceUpdateKind::Removed
        ) {
            source
                .sources
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&owner);
        }
        if kind == WorkspaceUpdateKind::Removed {
            source
                .rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&owner);
            source.publish();
            return;
        }
        if let Some((row, revision)) = crate::projection::workspace(world, owner) {
            let database = world
                .get_component::<Workspace>(owner)
                .unwrap()
                .database
                .clone();
            let mut rows = source.rows.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(current) = rows.get_mut(&owner) {
                current.project(database, row, revision);
            } else {
                rows.insert(
                    owner,
                    WorkspaceView {
                        database,
                        revision,
                        row,
                    },
                );
            }
            drop(rows);
            source.publish();
        }
        if kind != WorkspaceUpdateKind::Installed {
            return;
        }
        let Some(database) = world
            .get_component::<Workspace>(owner)
            .map(|w| w.database.clone())
        else {
            return;
        };
        {
            let watches = source.sources.lock().unwrap_or_else(|e| e.into_inner());
            if watches
                .get(&owner)
                .is_some_and(|w| w.database.same_connection(&database))
            {
                return;
            }
        }
        let Some(state) = world.get_resource::<crate::types::DtoState>().cloned() else {
            return;
        };
        let mut changes = database.subscribe_changes();
        let active = Arc::new(AtomicBool::new(true));
        let cancel = tokio_util::sync::CancellationToken::new();
        source
            .sources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                owner,
                WorkspaceWatch {
                    database: database.clone(),
                    active: active.clone(),
                    cancel: cancel.clone(),
                },
            );
        let weak = Arc::downgrade(&source);
        let guard = state.lifetime.track();
        let lifetime = state.lifetime;
        world.get_resource::<async_runtime_plugin::AsyncRuntimeHandle>().unwrap().spawn(async move{
   let _guard=guard;
   loop {
    let value=changes.borrow_and_update().clone();
    if !active.load(Ordering::Acquire){break;}
    let Some(snapshot)=value else{break;};
    if let Some(views)=weak.upgrade(){
     if active.load(Ordering::Acquire){let mut rows=views.rows.lock().unwrap_or_else(|e|e.into_inner());if active.load(Ordering::Acquire) && let Some(current)=rows.get_mut(&owner){current.apply_snapshot(&database, snapshot);}drop(rows);views.publish();}
    }else{break;}
    tokio::select!{_=cancel.cancelled()=>break,_=lifetime.cancelled()=>break,result=changes.changed()=>if result.is_err(){break;}}
   }
  });
    });
    WorkspaceSyncBinding { updates, views }
}
pub(crate) fn install_workspace_monitor(world: &mut World) {
    let Some(binding) = world.get_resource::<WorkspaceSyncBinding>().cloned() else {
        return;
    };
    for owner in world.query_with::<Workspace>().result() {
        binding
            .updates
            .notify(world, owner, WorkspaceUpdateKind::Installed);
    }
    binding.views.publish();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_snapshot_cannot_rollback_alias_or_override_current_running_state() {
        let root = tempfile::tempdir().unwrap();
        for kind in ["normal", "temporary", "archived"] {
            std::fs::create_dir_all(root.path().join("agents").join(kind)).unwrap();
        }
        let database = workspace_core::WorkspaceDatabase::create(
            root.path().join("workspace.sqlite"),
            workspace_core::WorkspaceMetadata {
                address: "/demo".into(),
                alias: "initial".into(),
                project_root: root.path().into(),
            },
        )
        .unwrap();
        let initial = database.subscribe_changes().borrow().clone().unwrap();
        let row = WorkspaceInfoDto {
            address: "/demo".into(),
            alias: initial.metadata.alias.clone(),
            project_root: root.path().to_string_lossy().into_owned(),
            status: "running".into(),
            agents: vec![],
        };
        let mut view = WorkspaceView {
            database: database.clone(),
            revision: initial.revision,
            row: row.clone(),
        };
        database.rename_workspace_alias("latest".into()).unwrap();
        let latest = database.subscribe_changes().borrow().clone().unwrap();
        view.apply_snapshot(&database, latest);
        let mut delayed = row;
        delayed.status = "deleting".into();
        view.project(database.clone(), delayed, initial.revision);
        assert_eq!(view.row.alias, "latest");
        assert_eq!(view.row.status, "deleting");
        view.apply_snapshot(&database, initial);
        assert_eq!(view.row.alias, "latest");
        assert_eq!(view.row.status, "deleting");
        let replacement = workspace_core::WorkspaceDatabase::open(database.path()).unwrap();
        let mut foreign = replacement.subscribe_changes().borrow().clone().unwrap();
        foreign.metadata.alias = "foreign".into();
        foreign.revision = u64::MAX;
        view.apply_snapshot(&replacement, foreign);
        assert_eq!(view.row.alias, "latest");
    }
}
