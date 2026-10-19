use std::{
    collections::HashSet,
    thread,
    time::{Duration, Instant},
};

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use client_plugin::ClientPlugin;
use config_plugin::{ConfigPlugin, MargatroidConfig, WebSocketMessageTarget};
use core_plugin::App;
use dto_plugin::DtoPlugin;
use futures_util::{SinkExt, StreamExt};
use log_plugin::LogPlugin;
use margatroid_protocol::{ClientMessage, ServerMessage};
use margatroid_types::{
    workspace_catalog::WorkspaceCatalogEntry, workspace_components::WorkspaceDefinition,
    workspace_events::*,
};
use resource_id_plugin::{ResourceID, ResourcePlugin};
use serde_json::json;
use server_plugin::{ServerHandle, ServerPlugin};
use tokio_tungstenite::tungstenite::Message;

#[test]
fn catalog_and_member_results_preserve_per_client_ids_through_internal_operations() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(LogPlugin::default().without_console().with_stream(8))
        .add_plugin(ServerPlugin::bind("127.0.0.1:0"))
        .add_plugin(ConfigPlugin::new(
            MargatroidConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                Default::default(),
                None,
                vec![WebSocketMessageTarget::Type("unmatched".into())],
                vec![WebSocketMessageTarget::Type("unmatched".into())],
            )
            .unwrap(),
        ))
        .add_plugin(ClientPlugin::default())
        .add_plugin(DtoPlugin::default());
    app.tick();
    let deadline = Instant::now() + Duration::from_secs(5);
    let address = loop {
        if let Some(address) = app
            .world()
            .get_resource::<ServerHandle>()
            .and_then(ServerHandle::local_address)
        {
            break address;
        }
        assert!(Instant::now() < deadline);
        thread::yield_now();
    };
    let clients: Vec<_> = (0..2).map(|index| thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
            let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws")).await.unwrap();
            let register = ClientMessage::register_connection_with_name("register", "catalog-test", format!("client-{index}"));
            socket.send(Message::Text(serde_json::to_string(&register).unwrap().into())).await.unwrap();
            let reply = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
            let Message::Text(reply) = reply else { panic!("expected registration reply") };
            assert!(matches!(serde_json::from_str::<ServerMessage>(&reply).unwrap(), ServerMessage::ConnectionRegistered { .. }));
            for kind in ["workspace.catalog.query", "workspace.catalog.refresh", "workspace.member.add", "workspace.member.remove", "workspace.create", "workspace.rename", "workspace.delete", "workspace.member.rename", "workspace.start", "workspace.paths.query", "driver.catalog.query", "driver.catalog.refresh"] {
                let message = match kind {
                    "workspace.paths.query" => json!({"path":format!("/client-{index}"),"cursor":"page-2"}),
                    "workspace.member.add" => json!({"workspace":"workspace:test/empty:latest","expected_revision":"revision","agent":{"name":"worker","driver":"driver:test/base:latest"}}),
                    "workspace.create" => json!({"name":"empty","project_root":"/project"}),
                    "workspace.rename" => json!({"workspace":"workspace:test/empty:latest","expected_revision":"revision","new_name":"renamed"}),
                    "workspace.delete" => json!({"workspace":"workspace:test/empty:latest","expected_revision":"revision"}),
                    "workspace.member.rename" => json!({"workspace":"workspace:test/empty:latest","expected_revision":"revision","agent":"agent:test/worker:latest","new_name":"renamed"}),
                    "workspace.member.remove" => json!({"workspace":"workspace:test/empty:latest","expected_revision":"revision","agent":"agent:test/worker:latest"}),
                    "workspace.start" => json!({"workspace":"workspace:test/empty:latest","expected_revision":"revision"}),
                    _ => json!({}),
                };
                socket.send(Message::Text(json!({"type":kind,"id":"same-network-id","message":message}).to_string().into())).await.unwrap();
                let reply = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
                let Message::Text(reply) = reply else { panic!("expected correlated reply") };
                let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(reply["id"], "same-network-id");
                match kind {
                    "workspace.paths.query" => {
                        assert_eq!(reply["type"], "workspace.paths");
                        assert_eq!(reply["path"], format!("/client-{index}"));
                        assert!(reply["parent"].is_null());
                        assert_eq!(reply["directories"][0]["name"], "child");
                        assert_eq!(reply["directories"][0]["path"], format!("/client-{index}/child"));
                        assert_eq!(reply["next_cursor"], "page-3");
                    }
                    "driver.catalog.query" => {
                        assert_eq!(reply["type"], "driver.catalog");
                        assert_eq!(reply["drivers"][0], "driver:test/base:latest");
                        assert_eq!(reply["errors"][0]["path"], "/drivers/broken");
                        assert_eq!(reply["errors"][0]["message"], "invalid package");
                    }
                    "driver.catalog.refresh" => {
                        assert_eq!(reply["type"], "request.failed");
                        assert_eq!(reply["error"]["kind"], "AgentDriverCatalogFailed");
                        assert_eq!(reply["error"]["message"], "scan failed");
                    }
                    "workspace.catalog.query" | "workspace.catalog.refresh" => {
                        assert_eq!(reply["type"], "workspace.catalog");
                        assert_eq!(reply["entries"][0]["revision"], "revision");
                    }
                    "workspace.delete" => {
                        assert_eq!(reply["type"], "workspace.member.result");
                        assert!(reply["entry"].is_null());
                        assert!(reply.get("error").is_none());
                    }
                    "workspace.start" => assert_eq!(reply["type"], "workspace.start_failed"),
                    _ => { assert_eq!(reply["type"], "workspace.member.result"); assert_eq!(reply["entry"]["revision"], "revision"); assert!(reply.get("error").is_none()); }
                }
            }
        });
    })).collect();
    let workspace_id = ResourceID::new("workspace", "test", "empty", None).unwrap();
    let entry = WorkspaceCatalogEntry {
        id: workspace_id.clone(),
        name: "empty".into(),
        path: "/registered/empty/workspace.yaml".into(),
        revision: "revision".into(),
        definition: Some(WorkspaceDefinition {
            id: workspace_id.clone(),
            name: "empty".into(),
            project_root: "/project".into(),
            agents: vec![],
        }),
        error: None,
    };
    let mut operations = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while clients.iter().any(|client| !client.is_finished()) {
        app.tick();
        let world = app.world();
        use margatroid_types::discovery::*;
        for event in world.event_reader::<QueryProjectDirectories>() {
            assert!(operations.insert(event.id.clone()));
            assert_eq!(event.cursor.as_deref(), Some("page-2"));
            let path = event.path.clone().unwrap();
            // A driver result with this directory operation ID must be ignored.
            world.send_event(AgentDriverCatalogResult {
                id: event.id.clone(),
                result: Err("wrong kind".into()),
            });
            world.send_event(QueryProjectDirectoriesResult {
                id: event.id.clone(),
                result: Ok(ProjectDirectoryPage {
                    path: path.clone(),
                    parent: None,
                    directories: vec![ProjectDirectoryEntry {
                        name: "child".into(),
                        path: path.join("child"),
                    }],
                    next_cursor: Some("page-3".into()),
                }),
            });
        }
        for event in world.event_reader::<QueryAgentDriverCatalog>() {
            assert!(operations.insert(event.id.clone()));
            world.send_event(QueryProjectDirectoriesResult {
                id: event.id.clone(),
                result: Err("wrong kind".into()),
            });
            world.send_event(AgentDriverCatalogResult {
                id: event.id.clone(),
                result: Ok(AgentDriverCatalogSnapshot {
                    drivers: vec![ResourceID::new("driver", "test", "base", None).unwrap()],
                    errors: vec![DiscoveryError {
                        path: "/drivers/broken".into(),
                        message: "invalid package".into(),
                    }],
                }),
            });
        }
        for event in world.event_reader::<RefreshAgentDriverCatalog>() {
            assert!(operations.insert(event.id.clone()));
            world.send_event(AgentDriverCatalogResult {
                id: event.id.clone(),
                result: Err("scan failed".into()),
            });
        }
        for event in world.event_reader::<QueryWorkspaceCatalog>() {
            assert!(operations.insert(event.id.clone()));
            world.send_event(QueryWorkspaceCatalogResult {
                id: event.id.clone(),
                result: Ok(vec![entry.clone()]),
            });
        }
        for event in world.event_reader::<RefreshWorkspaceCatalog>() {
            assert!(operations.insert(event.id.clone()));
            world.send_event(RefreshWorkspaceCatalogResult {
                id: event.id.clone(),
                result: Ok(vec![entry.clone()]),
            });
        }
        for event in world.event_reader::<AddWorkspaceMember>() {
            assert!(operations.insert(event.id.clone()));
            assert_eq!(event.workspace_id, workspace_id);
            assert_eq!(event.expected_revision, "revision");
            assert!(event.member.home_root.is_none());
            world.send_event(AddWorkspaceMemberResult {
                id: event.id.clone(),
                result: Ok(entry.clone()),
            });
        }
        for event in world.event_reader::<RemoveWorkspaceMember>() {
            assert!(operations.insert(event.id.clone()));
            assert_eq!(event.expected_revision, "revision");
            world.send_event(RemoveWorkspaceMemberResult {
                id: event.id.clone(),
                result: Ok(entry.clone()),
            });
        }
        for event in world.event_reader::<CreateWorkspace>() {
            assert!(operations.insert(event.id.clone()));
            assert_eq!(event.name, "empty");
            assert_eq!(event.project_root, std::path::PathBuf::from("/project"));
            // A result with the right ID but wrong operation must not consume the pending create.
            world.send_event(RemoveWorkspaceMemberResult {
                id: event.id.clone(),
                result: Ok(entry.clone()),
            });
            world.send_event(CreateWorkspaceResult {
                id: event.id.clone(),
                result: Ok(entry.clone()),
            });
        }
        for event in world.event_reader::<RenameWorkspace>() {
            assert!(operations.insert(event.id.clone()));
            assert_eq!(event.workspace_id, workspace_id);
            assert_eq!(event.expected_revision, "revision");
            assert_eq!(event.new_name, "renamed");
            world.send_event(RenameWorkspaceResult {
                id: event.id.clone(),
                result: Ok(entry.clone()),
            });
        }
        for event in world.event_reader::<DeleteWorkspace>() {
            assert!(operations.insert(event.id.clone()));
            assert_eq!(event.expected_revision, "revision");
            world.send_event(DeleteWorkspaceResult {
                id: event.id.clone(),
                result: Ok(margatroid_types::workspace_catalog::WorkspaceDeletion {
                    id: workspace_id.clone(),
                    path: "/registered/empty/workspace.yaml".into(),
                    removed_path: "/private/recovered/empty".into(),
                }),
            });
        }
        for event in world.event_reader::<RenameWorkspaceMember>() {
            assert!(operations.insert(event.id.clone()));
            assert_eq!(event.expected_revision, "revision");
            assert_eq!(event.new_name, "renamed");
            assert_eq!(event.member_id.to_string(), "agent:test/worker:latest");
            world.send_event(RenameWorkspaceMemberResult {
                id: event.id.clone(),
                result: Ok(entry.clone()),
            });
        }
        for event in world.event_reader::<StartRegisteredWorkspace>() {
            assert!(operations.insert(event.id.clone()));
            assert_eq!(event.workspace_id, workspace_id);
            assert_eq!(event.expected_revision, "revision");
            world.send_event(StartRegisteredWorkspaceResult {
                id: event.id.clone(),
                result: Err(margatroid_types::workspace_components::WorkspaceError::new(
                    margatroid_types::workspace_components::WorkspaceErrorKind::InvalidRequest,
                    "test lifecycle rejection",
                )),
            });
        }
        assert!(
            Instant::now() < deadline,
            "catalog correlation test timed out"
        );
        thread::yield_now();
    }
    for client in clients {
        client.join().unwrap();
    }
    assert_eq!(operations.len(), 24);
    assert!(!operations.contains("same-network-id"));
}
