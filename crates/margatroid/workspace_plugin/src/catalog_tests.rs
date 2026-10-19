use crate::*;
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Event};
use margatroid_types::agent_events::{StopAgent, StopAgentResult};
use margatroid_types::workspace_catalog::{
    WorkspaceCatalogEntry, WorkspaceCatalogHandle, WorkspaceCatalogStore, WorkspaceDeletion,
};
use margatroid_types::workspace_execution::spawn_workspace_execution;
use resource_id_plugin::{ResourceID, ResourcePlugin, WorldResourceExt};
use std::sync::{Arc, Mutex};

struct Store {
    entry: Mutex<WorkspaceCatalogEntry>,
    fail_save: bool,
    omit_listing: std::sync::atomic::AtomicBool,
    main_thread: std::thread::ThreadId,
}
impl WorkspaceCatalogStore for Store {
    fn list(&self) -> Result<Vec<WorkspaceCatalogEntry>, WorkspaceError> {
        assert_ne!(std::thread::current().id(), self.main_thread);
        if self.omit_listing.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(vec![]);
        }
        Ok(vec![self.entry.lock().unwrap().clone()])
    }
    fn load(
        &self,
        id: &ResourceID,
        revision: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        assert_ne!(std::thread::current().id(), self.main_thread);
        let entry = self.entry.lock().unwrap();
        if &entry.id != id || revision != entry.revision {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceBusy,
                "revision conflict",
            ));
        }
        Ok(entry.clone())
    }
    fn save(
        &self,
        definition: &WorkspaceDefinition,
        revision: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        assert_ne!(std::thread::current().id(), self.main_thread);
        if self.fail_save {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::InvalidDefinition,
                "disk failure",
            ));
        }
        let mut entry = self.entry.lock().unwrap();
        if revision.is_empty() {
            if entry.id == definition.id {
                return Err(WorkspaceError::new(
                    WorkspaceErrorKind::DuplicateWorkspace,
                    "already exists",
                ));
            }
            entry.id = definition.id.clone();
            entry.name = definition.name.clone();
            entry.path = format!("/catalog/{}/workspace.yaml", definition.name).into();
        } else {
            assert_eq!(entry.revision, revision);
        }
        entry.revision.push('x');
        entry.definition = Some(definition.clone());
        Ok(entry.clone())
    }
    fn rename(
        &self,
        id: &ResourceID,
        revision: &str,
        new_name: &str,
        definition: &WorkspaceDefinition,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        assert_ne!(std::thread::current().id(), self.main_thread);
        let mut entry = self.entry.lock().unwrap();
        assert_eq!(&entry.id, id);
        assert_eq!(entry.revision, revision);
        // The store accepts the exact source snapshot, not a pre-readdressed target.
        assert_eq!(entry.definition.as_ref(), Some(definition));
        assert_eq!(&definition.id, id);
        let source = entry.path.parent().unwrap().to_path_buf();
        let destination = source.with_file_name(new_name);
        let mut target = definition.clone();
        target.id = margatroid_types::workspace_catalog::workspace_resource_id(new_name).unwrap();
        target.name = new_name.into();
        for member in &mut target.agents {
            member.id = ResourceID::new("agent", new_name, &member.name, None).unwrap();
            member.history_path =
                destination.join(member.history_path.strip_prefix(&source).unwrap());
        }
        entry.id = target.id.clone();
        entry.name = new_name.into();
        entry.path = destination.join("workspace.yaml");
        entry.definition = Some(target);
        entry.revision.push('r');
        Ok(entry.clone())
    }
    fn delete(&self, id: &ResourceID, revision: &str) -> Result<WorkspaceDeletion, WorkspaceError> {
        assert_ne!(std::thread::current().id(), self.main_thread);
        let entry = self.entry.lock().unwrap();
        assert_eq!(&entry.id, id);
        assert_eq!(entry.revision, revision);
        self.omit_listing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(WorkspaceDeletion {
            id: id.clone(),
            path: entry.path.parent().unwrap().into(),
            removed_path: "/catalog/.trash/operation".into(),
        })
    }
    fn rename_member(
        &self,
        id: &ResourceID,
        revision: &str,
        member: &ResourceID,
        name: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        assert_ne!(std::thread::current().id(), self.main_thread);
        if self.fail_save {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::InvalidDefinition,
                "migration failed; recovery_required",
            ));
        }
        let mut entry = self.entry.lock().unwrap();
        assert_eq!(&entry.id, id);
        assert_eq!(entry.revision, revision);
        let target = entry
            .definition
            .as_mut()
            .unwrap()
            .agents
            .iter_mut()
            .find(|m| &m.id == member)
            .unwrap();
        target.id = ResourceID::new("agent", id.name(), name, None).unwrap();
        target.name = name.into();
        target.history_path = format!("/catalog/{}/agents/{name}/state.sqlite", id.name()).into();
        entry.revision.push('m');
        Ok(entry.clone())
    }
}
fn fixture(members: bool, fail_save: bool) -> (App, Arc<Store>) {
    let definition = WorkspaceDefinition {
        id: ResourceID::parse("workspace:local/test").unwrap(),
        name: "test".into(),
        project_root: "/project".into(),
        agents: if members {
            vec![WorkspaceAgentDefinition {
                id: ResourceID::parse("agent:test/a").unwrap(),
                name: "a".into(),
                driver: ResourceID::parse("driver:local/shared").unwrap(),
                home_root: "/home/test".into(),
                history_path: "/catalog/test/agents/a/state.sqlite".into(),
            }]
        } else {
            vec![]
        },
    };
    let store = Arc::new(Store {
        entry: Mutex::new(WorkspaceCatalogEntry {
            id: definition.id.clone(),
            name: definition.name.clone(),
            path: "/catalog/test/workspace.yaml".into(),
            revision: "r1".into(),
            definition: Some(definition),
            error: None,
        }),
        fail_save,
        omit_listing: std::sync::atomic::AtomicBool::new(false),
        main_thread: std::thread::current().id(),
    });
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(WorkspacePlugin::default());
    app.world_mut()
        .insert_resource(WorkspaceCatalogHandle(store.clone()));
    (app, store)
}
async fn wait<E: Event + Clone>(app: &mut App) -> E {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            app.world_mut().advance_event_frame();
            crate::catalog::system(app.world_mut());
            if let Some(event) = app.world().event_reader::<E>().into_iter().next().cloned() {
                return event;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn duplicate_edit_and_refresh_cannot_release_active_operation() {
    let (mut app, store) = fixture(false, false);
    let workspace = store.entry.lock().unwrap().id.clone();
    let request = AddWorkspaceMember {
        id: "edit".into(),
        workspace_id: workspace.clone(),
        expected_revision: "r1".into(),
        member: WorkspaceMemberSpec {
            name: "a".into(),
            driver: ResourceID::parse("driver:local/shared").unwrap(),
            home_root: Some("/home/test".into()),
        },
    };
    app.world().send_event(request.clone());
    app.world_mut().advance_event_frame();
    crate::catalog::system(app.world_mut());
    app.world().send_event(request);
    app.world().send_event(RefreshWorkspaceCatalog {
        id: "refresh".into(),
    });
    app.world_mut().advance_event_frame();
    crate::catalog::system(app.world_mut());
    let rejected = wait::<RefreshWorkspaceCatalogResult>(&mut app).await;
    assert_eq!(
        rejected.result.unwrap_err().kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    let result = loop {
        let result = wait::<AddWorkspaceMemberResult>(&mut app).await;
        if result.result.is_ok() {
            break result;
        }
    };
    assert_eq!(result.result.unwrap().definition.unwrap().agents.len(), 1);
    assert!(app.world().workspace_list().is_empty());
    assert_eq!(
        store
            .entry
            .lock()
            .unwrap()
            .definition
            .as_ref()
            .unwrap()
            .agents
            .len(),
        1
    );
}
#[tokio::test]
async fn failed_removal_save_retains_definition_but_reports_actual_stopped_member() {
    let (mut app, store) = fixture(true, true);
    let definition = store.entry.lock().unwrap().definition.clone().unwrap();
    let (entry, close) = spawn_workspace_execution().unwrap();
    let workspace = app.world_mut().spawn();
    app.world_mut()
        .insert_library(
            workspace,
            Workspace::new(
                definition.clone(),
                WorkspaceState::Running,
                entry,
                close.clone(),
            ),
        )
        .unwrap();
    let agent = app.world_mut().spawn();
    app.world_mut()
        .get_component_mut::<Workspace>(workspace)
        .unwrap()
        .insert_agent(definition.agents[0].id.clone(), agent);
    app.world().send_event(RemoveWorkspaceMember {
        id: "remove".into(),
        workspace_id: definition.id.clone(),
        expected_revision: "r1".into(),
        member_id: definition.agents[0].id.clone(),
    });
    let stop = wait::<StopAgent>(&mut app).await;
    assert_eq!(stop.agent, agent);
    assert_eq!(
        store
            .entry
            .lock()
            .unwrap()
            .definition
            .as_ref()
            .unwrap()
            .agents
            .len(),
        1
    );
    app.world().send_event(StopAgentResult {
        id: stop.id,
        result: Ok(agent),
    });
    let result = wait::<RemoveWorkspaceMemberResult>(&mut app).await;
    assert!(result
        .result
        .unwrap_err()
        .message()
        .contains("persisted definition retained"));
    let runtime = app.world().get_component::<Workspace>(workspace).unwrap();
    assert!(matches!(runtime.state(), WorkspaceState::Running));
    assert_eq!(runtime.iter().count(), 0);
    assert_eq!(runtime.definition().agents.len(), 1);
    assert_eq!(
        store
            .entry
            .lock()
            .unwrap()
            .definition
            .as_ref()
            .unwrap()
            .agents
            .len(),
        1
    );
    close.close().await.unwrap();
}
#[tokio::test]
async fn refresh_diagnoses_external_edits_without_stopping_runtime() {
    let (mut app, store) = fixture(false, false);
    let definition = store.entry.lock().unwrap().definition.clone().unwrap();
    let (entry, close) = spawn_workspace_execution().unwrap();
    let workspace = app.world_mut().spawn();
    app.world_mut()
        .insert_library(
            workspace,
            Workspace::new(definition, WorkspaceState::Running, entry, close.clone()),
        )
        .unwrap();
    store
        .entry
        .lock()
        .unwrap()
        .definition
        .as_mut()
        .unwrap()
        .project_root = "/changed".into();
    app.world().send_event(RefreshWorkspaceCatalog {
        id: "refresh".into(),
    });
    let result = wait::<RefreshWorkspaceCatalogResult>(&mut app).await;
    assert!(result.result.unwrap()[0]
        .error
        .as_ref()
        .unwrap()
        .contains("explicitly stop"));
    assert_eq!(
        app.world()
            .get_component::<Workspace>(workspace)
            .unwrap()
            .project_root(),
        std::path::Path::new("/project")
    );
    close.close().await.unwrap();
}

#[tokio::test]
async fn refresh_keeps_live_workspace_visible_when_persistent_home_disappears() {
    let (mut app, store) = fixture(false, false);
    let previous = store.entry.lock().unwrap().clone();
    app.world_mut()
        .get_resource_mut::<margatroid_types::workspace_catalog::WorkspaceCatalog>()
        .unwrap()
        .0
        .push(previous.clone());
    let (entry, close) = spawn_workspace_execution().unwrap();
    let workspace = app.world_mut().spawn();
    app.world_mut()
        .insert_library(
            workspace,
            Workspace::new(
                previous.definition.unwrap(),
                WorkspaceState::Running,
                entry,
                close.clone(),
            ),
        )
        .unwrap();
    store
        .omit_listing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    app.world().send_event(RefreshWorkspaceCatalog {
        id: "refresh-missing".into(),
    });
    let entries = wait::<RefreshWorkspaceCatalogResult>(&mut app)
        .await
        .result
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, previous.id);
    assert_eq!(entries[0].path, previous.path);
    assert!(entries[0]
        .error
        .as_ref()
        .unwrap()
        .contains("home is missing"));
    assert!(matches!(
        app.world()
            .get_component::<Workspace>(workspace)
            .unwrap()
            .state(),
        WorkspaceState::Running
    ));
    close.close().await.unwrap();
}

#[tokio::test]
async fn create_is_empty_stopped_and_cannot_overwrite() {
    let (mut app, _) = fixture(false, false);
    app.world().send_event(CreateWorkspace {
        id: "create".into(),
        name: "new".into(),
        project_root: "/project".into(),
    });
    let entry = wait::<CreateWorkspaceResult>(&mut app)
        .await
        .result
        .unwrap();
    assert!(entry.definition.unwrap().agents.is_empty());
    assert!(app.world().workspace_list().is_empty());
    app.world().send_event(CreateWorkspace {
        id: "duplicate".into(),
        name: "new".into(),
        project_root: "/project".into(),
    });
    assert_eq!(
        wait::<CreateWorkspaceResult>(&mut app)
            .await
            .result
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::DuplicateWorkspace
    );
}

#[tokio::test]
async fn stopped_workspace_rename_then_delete_updates_catalog() {
    let (mut app, store) = fixture(true, false);
    app.world().send_event(RefreshWorkspaceCatalog {
        id: "refresh".into(),
    });
    wait::<RefreshWorkspaceCatalogResult>(&mut app)
        .await
        .result
        .unwrap();
    let old = store.entry.lock().unwrap().clone();
    app.world().send_event(RenameWorkspace {
        id: "rename".into(),
        workspace_id: old.id.clone(),
        expected_revision: old.revision,
        new_name: "next".into(),
    });
    let next = wait::<RenameWorkspaceResult>(&mut app)
        .await
        .result
        .unwrap();
    assert_eq!(next.id.to_string(), "workspace:local/next:latest");
    assert_eq!(
        next.definition.as_ref().unwrap().agents[0].id.to_string(),
        "agent:next/a:latest"
    );
    let catalog = &app
        .world()
        .get_resource::<margatroid_types::workspace_catalog::WorkspaceCatalog>()
        .unwrap()
        .0;
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].id, next.id);
    app.world().send_event(DeleteWorkspace {
        id: "delete".into(),
        workspace_id: next.id,
        expected_revision: next.revision,
    });
    let deleted = wait::<DeleteWorkspaceResult>(&mut app)
        .await
        .result
        .unwrap();
    assert_eq!(
        deleted.removed_path,
        std::path::Path::new("/catalog/.trash/operation")
    );
    assert!(app
        .world()
        .get_resource::<margatroid_types::workspace_catalog::WorkspaceCatalog>()
        .unwrap()
        .0
        .is_empty());
}

#[tokio::test]
async fn live_member_rename_drains_before_commit_and_tracks_restart_failure() {
    use margatroid_types::agent_driver_events::{LoadAgentDriver, LoadAgentDriverResult};
    let (mut app, store) = fixture(true, false);
    let definition = store.entry.lock().unwrap().definition.clone().unwrap();
    let (entry, close) = spawn_workspace_execution().unwrap();
    let workspace = app.world_mut().spawn();
    app.world_mut()
        .insert_library(
            workspace,
            Workspace::new(
                definition.clone(),
                WorkspaceState::Running,
                entry,
                close.clone(),
            ),
        )
        .unwrap();
    let agent = app.world_mut().spawn();
    app.world_mut()
        .get_component_mut::<Workspace>(workspace)
        .unwrap()
        .insert_agent(definition.agents[0].id.clone(), agent);
    app.world().send_event(RenameWorkspaceMember {
        id: "rename".into(),
        workspace_id: definition.id.clone(),
        expected_revision: "r1".into(),
        member_id: definition.agents[0].id.clone(),
        new_name: "b".into(),
    });
    let stop = wait::<StopAgent>(&mut app).await;
    assert_eq!(stop.agent, agent);
    assert_eq!(store.entry.lock().unwrap().revision, "r1");
    assert!(app
        .world()
        .event_reader::<LoadAgentDriver>()
        .into_iter()
        .next()
        .is_none());
    app.world().send_event(StopAgentResult {
        id: stop.id,
        result: Ok(agent),
    });
    let load = wait::<LoadAgentDriver>(&mut app).await;
    assert_eq!(
        store
            .entry
            .lock()
            .unwrap()
            .definition
            .as_ref()
            .unwrap()
            .agents[0]
            .name,
        "b"
    );
    assert_eq!(
        app.world()
            .get_component::<Workspace>(workspace)
            .unwrap()
            .iter()
            .count(),
        0
    );
    app.world().send_event(DeleteWorkspace {
        id: "busy".into(),
        workspace_id: definition.id,
        expected_revision: "r1m".into(),
    });
    assert_eq!(
        wait::<DeleteWorkspaceResult>(&mut app)
            .await
            .result
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    let mut drivers = resource_id_plugin::ResourceSet::new();
    drivers.insert(
        load.resource_id.clone(),
        margatroid_types::resource_components::AgentDriver {
            base_driver: Arc::from(""),
            origin: Arc::new("/driver/driver.lua".into()),
            dependencies: Arc::from([]),
            model: margatroid_types::resource_components::AgentDriverModelConfig {
                model: "test".into(),
                temperature: None,
                max_output_tokens: None,
                top_p: None,
                stop: vec![],
            },
        },
    );
    app.world_mut().insert_component(workspace, drivers);
    app.world().send_event(LoadAgentDriverResult {
        id: load.id,
        target: workspace,
        resource_id: load.resource_id,
        result: Ok(()),
    });
    let start = wait::<margatroid_types::agent_events::StartAgent>(&mut app).await;
    assert_eq!(start.resource_id.to_string(), "agent:test/b:latest");
    assert_eq!(
        start.definition.history_path,
        std::path::Path::new("/catalog/test/agents/b/state.sqlite")
    );
    app.world().send_event(RefreshWorkspaceCatalog {
        id: "refresh-during-restart".into(),
    });
    assert_eq!(
        wait::<RefreshWorkspaceCatalogResult>(&mut app)
            .await
            .result
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    app.world()
        .send_event(margatroid_types::agent_events::StartAgentResult {
            id: start.id,
            result: Err(margatroid_types::agent_events::AgentError::new(
                margatroid_types::agent_events::AgentErrorKind::DriverStartFailed,
                "runtime startup failed",
            )),
        });
    let result = wait::<RenameWorkspaceMemberResult>(&mut app)
        .await
        .result
        .unwrap();
    assert_eq!(result.revision, "r1m");
    let runtime = app.world().get_component::<Workspace>(workspace).unwrap();
    assert_eq!(runtime.definition().agents[0].name, "b");
    assert!(runtime
        .member_failures()
        .contains_key(&ResourceID::parse("agent:test/b").unwrap()));
    close.close().await.unwrap();
}

#[tokio::test]
async fn member_rename_commit_failure_keeps_old_definition_but_not_stopped_reference() {
    // A configured but non-live member is migrated without bringing up a runtime.
    let (mut stopped, stopped_store) = fixture(true, false);
    let old = stopped_store.entry.lock().unwrap().clone();
    stopped.world().send_event(RenameWorkspaceMember {
        id: "stopped-rename".into(),
        workspace_id: old.id,
        expected_revision: old.revision,
        member_id: old.definition.unwrap().agents[0].id.clone(),
        new_name: "b".into(),
    });
    let renamed = wait::<RenameWorkspaceMemberResult>(&mut stopped)
        .await
        .result
        .unwrap();
    assert_eq!(renamed.definition.unwrap().agents[0].name, "b");
    assert!(stopped.world().workspace_list().is_empty());
    assert!(stopped
        .world()
        .event_reader::<margatroid_types::agent_driver_events::LoadAgentDriver>()
        .into_iter()
        .next()
        .is_none());
    let (mut app, store) = fixture(true, true);
    let definition = store.entry.lock().unwrap().definition.clone().unwrap();
    let (entry, close) = spawn_workspace_execution().unwrap();
    let workspace = app.world_mut().spawn();
    app.world_mut()
        .insert_library(
            workspace,
            Workspace::new(
                definition.clone(),
                WorkspaceState::Running,
                entry,
                close.clone(),
            ),
        )
        .unwrap();
    let agent = app.world_mut().spawn();
    app.world_mut()
        .get_component_mut::<Workspace>(workspace)
        .unwrap()
        .insert_agent(definition.agents[0].id.clone(), agent);
    app.world().send_event(RenameWorkspaceMember {
        id: "rename".into(),
        workspace_id: definition.id.clone(),
        expected_revision: "r1".into(),
        member_id: definition.agents[0].id.clone(),
        new_name: "b".into(),
    });
    let stop = wait::<StopAgent>(&mut app).await;
    app.world().send_event(StopAgentResult {
        id: stop.id,
        result: Ok(agent),
    });
    let error = wait::<RenameWorkspaceMemberResult>(&mut app)
        .await
        .result
        .unwrap_err();
    assert!(error.message().contains("runtime was stopped"));
    assert!(error.message().contains("recovery_required"));
    let runtime = app.world().get_component::<Workspace>(workspace).unwrap();
    assert_eq!(runtime.iter().count(), 0);
    assert_eq!(runtime.definition(), &definition);
    assert_eq!(store.entry.lock().unwrap().revision, "r1");
    close.close().await.unwrap();
}
