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
    rows: Mutex<BTreeMap<Entity, WorkspaceInfoDto>>,
    view: Arc<Mutex<(u64, Vec<WorkspaceInfoDto>)>>,
    sync: SyncContext<Vec<WorkspaceInfoDto>>,
    sources: Mutex<BTreeMap<Entity, WorkspaceWatch>>,
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
        let mut values: Vec<_> = rows.values().cloned().collect();
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
        if let Some(row) = crate::projection::workspaces(world)
            .into_iter()
            .find(|row| crate::projection::address(world, owner).as_ref() == Some(&row.address))
        {
            source
                .rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(owner, row);
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
                    database,
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
     if active.load(Ordering::Acquire){let mut rows=views.rows.lock().unwrap_or_else(|e|e.into_inner());if let Some(row)=rows.get_mut(&owner){row.agents=snapshot.agents.into_iter().map(crate::projection::workspace_agent).collect();}drop(rows);views.publish();}
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
