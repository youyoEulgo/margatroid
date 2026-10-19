use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use config_plugin::{ConfigPlugin, LogLevel, MargatroidConfig, WebSocketMessageTarget};
use core_plugin::App;
use dto_plugin::{DtoPlugin, WebSocketMessageSend};
use log_plugin::LogPlugin;
use margatroid_protocol::ServerMessage;
use server_plugin::ServerPlugin;

#[test]
fn member_failure_events_publish_each_full_context_to_configured_logs_only() {
    use app_runtime_plugin::WorldEventExt;
    use margatroid_types::{
        workspace_components::{
            Workspace, WorkspaceAgentDefinition, WorkspaceDefinition, WorkspaceError,
            WorkspaceErrorKind, WorkspaceMemberFailure, WorkspaceState,
        },
        workspace_events::WorkspaceMemberStartFailed,
        workspace_execution::spawn_workspace_execution,
    };
    use resource_id_plugin::ResourceID;
    let mut app = App::new();
    let targets = vec![
        WebSocketMessageTarget::Type("operator".into()),
        WebSocketMessageTarget::Name("audit".into()),
    ];
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(LogPlugin::default().without_console().with_stream(8))
        .add_plugin(ServerPlugin::bind("127.0.0.1:0"))
        .add_plugin(ConfigPlugin::new(
            MargatroidConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                LogLevel::default(),
                None,
                targets.clone(),
                vec![WebSocketMessageTarget::Type("state".into())],
            )
            .unwrap(),
        ))
        .add_plugin(DtoPlugin::default());
    let workspace = app.world_mut().spawn();
    let workspace_id = ResourceID::new("workspace", "test", "partial", None).unwrap();
    let driver = ResourceID::new("agent-driver", "test", "broken", None).unwrap();
    let error = WorkspaceError::new(
        WorkspaceErrorKind::AgentDriverLoadFailed,
        "原始错误: missing dependency",
    );
    let (entry, close) = spawn_workspace_execution().unwrap();
    let mut component = Workspace::new(
        WorkspaceDefinition {
            id: workspace_id.clone(),
            name: "partial".into(),
            project_root: "/tmp/partial".into(),
            agents: ["manager", "worker"]
                .into_iter()
                .map(|member| WorkspaceAgentDefinition {
                    id: ResourceID::new("agent", "test", member, None).unwrap(),
                    name: member.into(),
                    driver: driver.clone(),
                    home_root: "/tmp/home".into(),
                    history_path: "/tmp/history".into(),
                })
                .collect(),
        },
        WorkspaceState::Running,
        entry,
        close.clone(),
    );
    for member in ["manager", "worker"] {
        component.record_member_failure(
            ResourceID::new("agent", "test", member, None).unwrap(),
            WorkspaceMemberFailure {
                driver: driver.clone(),
                error: error.clone(),
            },
        );
        app.world().send_event(WorkspaceMemberStartFailed {
            workspace,
            workspace_id: workspace_id.clone(),
            agent: ResourceID::new("agent", "test", member, None).unwrap(),
            driver: driver.clone(),
            error: error.clone(),
        });
    }
    app.world_mut().insert_component(workspace, component);
    app.tick();
    app.tick();
    assert!(app.world().event_reader::<WebSocketMessageSend>().into_iter().any(|event| {
        matches!(serde_json::from_value::<ServerMessage>(event.message.clone()).unwrap(),
            ServerMessage::StateSync { state } if state.workspaces.len() == 1
                && state.workspaces[0].member_failures.len() == 2
                && state.workspaces[0].member_failures.iter().all(|failure| failure.error == error.to_string()))
    }));
    let messages: Vec<_> = app
        .world()
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .filter(|event| event.message["record"]["target"] == "dto_plugin::workspace")
        .collect();
    assert_eq!(messages.len(), 4);
    for member in ["manager", "worker"] {
        for target in &targets {
            let record = messages
                .iter()
                .find(|event| {
                    &event.target == target
                        && event.message["record"]["message"]
                            .as_str()
                            .unwrap()
                            .contains(&format!("agent:test/{member}:latest"))
                })
                .expect("each member must reach each configured logs target");
            let ServerMessage::Log { record } =
                serde_json::from_value(record.message.clone()).unwrap()
            else {
                panic!("expected log")
            };
            for (key, value) in [
                ("workspace", workspace_id.to_string()),
                ("workspace_entity", format!("{workspace:?}")),
                ("driver", driver.to_string()),
                ("error", error.to_string()),
            ] {
                assert!(record
                    .fields
                    .iter()
                    .any(|field| field.name == key && field.value == value));
            }
        }
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(close.close())
        .unwrap();
}

#[test]
fn documented_public_api_publishes_state_for_the_configured_frontend_type() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(LogPlugin::default().without_console().with_stream(8))
        .add_plugin(ServerPlugin::bind("127.0.0.1:0"))
        .add_plugin(ConfigPlugin::new(
            MargatroidConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                LogLevel::default(),
                None,
                vec![WebSocketMessageTarget::Broadcast],
                vec![WebSocketMessageTarget::Type("browser".into())],
            )
            .unwrap(),
        ))
        .add_plugin(DtoPlugin::default());

    app.tick();
    app.tick();

    let messages = app
        .world()
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .collect::<Vec<_>>();
    assert!(messages.iter().any(|message| {
        message.target == WebSocketMessageTarget::Type("browser".into())
            && matches!(serde_json::from_value::<ServerMessage>(message.message.clone()).unwrap(),
                ServerMessage::StateSync { state } if state.workspaces.is_empty() && state.agents.is_empty() && state.histories.is_empty())
    }));

    app.tick();
    app.tick();
    assert!(app
        .world()
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .all(|message| message.message["type"] != "state.sync"));
}
