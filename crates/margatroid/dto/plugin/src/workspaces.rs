use crate::probes::{state_source, update};
use client_core::{ClientFrame, ClientPrepareError, Clients, SyncContext, SyncKey};
use core_plugin::{Entity, World};
use margatroid_protocol::*;
use notify::{EventKind, RecursiveMode, Watcher};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use workspace_core::{
    Workspace, WorkspaceDefinitionSnapshot, WorkspaceDefinitions, WorkspaceUpdateContext,
};

#[derive(Clone)]
struct LiveWorkspace {
    definition: Option<workspace_core::WorkspaceDefinition>,
    status: String,
    address: Option<String>,
    error: Option<ProtocolErrorDto>,
}
struct Catalog {
    definitions: BTreeMap<PathBuf, workspace_core::WorkspaceDefinition>,
    running: BTreeMap<PathBuf, LiveWorkspace>,
    definition_error: Option<String>,
}
struct WorkspaceViews {
    catalog: Mutex<Catalog>,
    view: Arc<Mutex<(u64, Vec<WorkspaceInfoDto>)>>,
    sync: SyncContext<Vec<WorkspaceInfoDto>>,
}
impl Drop for WorkspaceViews {
    fn drop(&mut self) {
        self.sync.close();
    }
}
impl WorkspaceViews {
    fn publish(&self) {
        let catalog = self.catalog.lock().unwrap();
        let mut definitions = catalog.definitions.clone();
        for (path, running) in &catalog.running {
            if let Some(definition) = &running.definition {
                definitions
                    .entry(path.clone())
                    .or_insert_with(|| definition.clone());
            }
        }
        let rows = definitions
            .into_iter()
            .map(|(path, definition)| {
                let live = catalog.running.get(&path);
                let error = live.and_then(|v| v.error.clone()).or_else(|| {
                    catalog
                        .definition_error
                        .as_ref()
                        .map(|message| ProtocolErrorDto::new("InvalidDefinition", message))
                });
                WorkspaceInfoDto {
                    path: path.to_string_lossy().into_owned(),
                    definition: definition_dto(definition),
                    status: live
                        .map(|v| v.status.clone())
                        .unwrap_or_else(|| "inactive".into()),
                    address: live.and_then(|v| v.address.clone()),
                    error,
                }
            })
            .collect();
        update(&self.view, &self.sync, rows);
        drop(catalog);
    }
}
fn definition_dto(value: workspace_core::WorkspaceDefinition) -> WorkspaceDefinitionDto {
    WorkspaceDefinitionDto {
        name: value.name,
        project_root: value.project_root.to_string_lossy().into_owned(),
        agents: value
            .members
            .into_iter()
            .map(|member| WorkspaceAgentDefinitionDto {
                name: member.name,
                driver: member.driver,
                home_root: member.home_root.to_string_lossy().into_owned(),
            })
            .collect(),
    }
}
#[derive(Clone)]
pub struct WorkspaceSyncBinding {
    pub updates: WorkspaceUpdateContext,
    views: Arc<WorkspaceViews>,
}
impl core_plugin::Resource for WorkspaceSyncBinding {}
pub fn workspace_sync_binding(
    clients: Clients,
    targets: &[config_core::OutboundTarget],
) -> WorkspaceSyncBinding {
    let view = Arc::new(Mutex::new((0, Vec::<WorkspaceInfoDto>::new())));
    let targets = crate::sync_selectors(targets);
    let sync = state_source(
        clients,
        targets,
        SyncKey {
            topic: "workspace.sync".into(),
            object: "all".into(),
        },
        &view,
        |workspaces| {
            encode_server_message(&ServerMessage::WorkspaceSync { workspaces })
                .map(|text| Arc::new(ClientFrame::Text(text)))
                .map_err(|_| ClientPrepareError::TooLarge)
        },
    );
    let views = Arc::new(WorkspaceViews {
        catalog: Mutex::new(Catalog {
            definitions: BTreeMap::new(),
            running: BTreeMap::new(),
            definition_error: None,
        }),
        view,
        sync,
    });
    let state = views.clone();
    let updates = WorkspaceUpdateContext::new(move |world: &mut World, _owner: Entity| {
        let running = world
            .query_with::<Workspace>()
            .result()
            .into_iter()
            .filter_map(|owner| {
                let item = world.get_component::<Workspace>(owner)?;
                Some((
                    item.definition_path.clone(),
                    LiveWorkspace {
                        definition: item.definition.clone(),
                        status: format!("{:?}", item.state).to_lowercase(),
                        address: crate::projection::address(world, owner),
                        error: item.error.as_ref().map(|error| {
                            ProtocolErrorDto::new(
                                format!("{:?}", error.kind),
                                error.message.clone(),
                            )
                        }),
                    },
                ))
            })
            .collect();
        state.catalog.lock().unwrap().running = running;
        state.publish();
    });
    WorkspaceSyncBinding { updates, views }
}
pub(crate) fn install_workspace_monitor(world: &mut World) {
    use async_runtime_plugin::WorldAsyncExt;
    let definitions = world
        .get_resource::<WorkspaceDefinitions>()
        .cloned()
        .unwrap_or_default();
    let binding = world
        .get_resource::<WorkspaceSyncBinding>()
        .expect("WorkspaceSyncBinding required")
        .clone();
    let initial = definitions.list().into_iter().collect();
    binding.views.catalog.lock().unwrap().definitions = initial;
    if let Some(owner) = world.query_with::<Workspace>().result().first().copied() {
        binding.updates.notify(world, owner);
    } else {
        binding.views.publish();
    }
    let weak = Arc::downgrade(&binding.views);
    definitions.observe(Arc::new(
        move |result: Result<WorkspaceDefinitionSnapshot, workspace_core::WorkspaceError>| {
            let Some(views) = weak.upgrade() else {
                return;
            };
            let mut catalog = views.catalog.lock().unwrap();
            match result {
                Ok(snapshot) => {
                    catalog.definitions = (*snapshot.entries).clone();
                    catalog.definition_error = None;
                }
                Err(error) => catalog.definition_error = Some(error.to_string()),
            };
            drop(catalog);
            views.publish();
        },
    ));
    let Some(source) = definitions.source().map(PathBuf::from) else {
        return;
    };
    let lifetime = world
        .get_resource::<crate::types::DtoState>()
        .unwrap()
        .lifetime
        .clone();
    let guard = lifetime.track();
    world.send_async_closure(
        app_runtime_plugin::RuntimePlugin::UPDATE,
        move || async move {
            let _guard = guard;
            if let Err(error) = watch_definitions(source, definitions, lifetime).await {
                tracing::warn!(reason=%error,"Workspace definition monitor stopped");
            }
            Ok::<_, crate::types::DtoAsyncError>(())
        },
    );
}
async fn watch_definitions(
    source: PathBuf,
    definitions: WorkspaceDefinitions,
    lifetime: axum_plugin::AxumLifetime,
) -> Result<(), String> {
    let dirty = Arc::new(AtomicBool::new(false));
    let notify = Arc::new(tokio::sync::Notify::new());
    let changes = dirty.clone();
    let wake = notify.clone();
    let target = source.clone();
    let watch_root = source
        .parent()
        .ok_or("definition source has no parent")?
        .to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let relevant = event.map_or(true, |event| definition_event(&target, &event));
        if relevant {
            changes.store(true, Ordering::Release);
            wake.notify_one();
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(&watch_root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    // Read after watcher installation to close the registration/read race.
    dirty.store(true, Ordering::Release);
    loop {
        while !dirty.swap(false, Ordering::AcqRel) {
            tokio::select! {_=lifetime.cancelled()=>return Ok(()),_=notify.notified()=>()}
        }
        loop {
            tokio::select! {_=lifetime.cancelled()=>return Ok(()),_=tokio::time::sleep(Duration::from_millis(100))=>()}
            if !dirty.swap(false, Ordering::AcqRel) {
                break;
            }
        }
        if lifetime.is_cancelled() {
            return Ok(());
        }
        let read = definitions.clone();
        let outcome = tokio::task::spawn_blocking(move || read.reload())
            .await
            .map_err(|e| e.to_string())?;
        if let Err(error) = outcome {
            tracing::warn!(reason=%error,"Workspace definition reload failed");
        }
    }
}

fn definition_event(source: &std::path::Path, event: &notify::Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| {
        if path == source {
            return true;
        }
        let Ok(relative) = path.strip_prefix(source) else {
            return false;
        };
        let depth = relative.components().count();
        (depth == 2
            && path
                .file_name()
                .is_some_and(|name| name == "workspace.yaml"))
            || (depth == 1
                && matches!(
                    event.kind,
                    EventKind::Create(_)
                        | EventKind::Remove(_)
                        | EventKind::Modify(notify::event::ModifyKind::Name(_))
                ))
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn definition_monitor_ignores_agent_database_and_output_file_changes() {
        let root = PathBuf::from("/tmp/config/workspaces");
        for path in [
            root.join("demo/agents/coder/state.sqlite"),
            root.join("demo/agents/coder/.tmp"),
            root.join("demo/state.sqlite.blocks/session/timeline.sqlite"),
        ] {
            let event = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Name(
                notify::event::RenameMode::To,
            )))
            .add_path(path);
            assert!(!definition_event(&root, &event));
        }
        for path in [root.join("demo/workspace.yaml"), root.join("demo")] {
            let event = notify::Event::new(EventKind::Create(notify::event::CreateKind::Any))
                .add_path(path);
            assert!(definition_event(&root, &event));
        }
    }
}
