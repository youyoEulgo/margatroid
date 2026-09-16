use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use client_plugin::{client_source, Client, ClientPlugin};
use config_plugin::{ConfigPlugin, LogLevel, MargatroidConfig, WebSocketMessageTarget};
use core_plugin::{App, World};
use dto_plugin::{DtoPlugin, WebSocketMessageSend};
use futures_util::{SinkExt, StreamExt};
use log_plugin::LogPlugin;
use margatroid_protocol::{ClientMessage, LogRecordDto, ServerMessage, WorkspaceReferenceDto};
use margatroid_types::{Message, ResourceId, RouteAgentMessage};
use resource_id_plugin::ResourceIdPlugin;
use server_plugin::{ServerHandle, ServerPlugin};

fn start(app: &mut App) -> SocketAddr {
    app.tick();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(address) = app
            .world()
            .get_resource::<ServerHandle>()
            .and_then(ServerHandle::local_address)
        {
            return address;
        }
        assert!(Instant::now() < deadline, "server startup timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

fn build_app() -> App {
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
                vec![WebSocketMessageTarget::Broadcast],
                vec![WebSocketMessageTarget::Broadcast],
                vec![WebSocketMessageTarget::Broadcast],
            )
            .unwrap(),
        ))
        .add_plugin(ResourceIdPlugin)
        .add_plugin(DtoPlugin::default())
        .add_plugin(ClientPlugin::default());
    app
}

fn client_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn first_client(app: &App) -> Option<(Client, ResourceId)> {
    let entity = app
        .world()
        .query_with::<Client>()
        .result()
        .into_iter()
        .next()?;
    let client = app.world().get_component::<Client>(entity)?.clone();
    let resource_id = app.world().get_component::<ResourceId>(entity)?.clone();
    Some((client, resource_id))
}

#[test]
fn registered_client_receives_messages_targeted_by_type() {
    let mut app = build_app();
    app.add_system(RuntimePlugin::UPDATE, |world: &mut World| {
        let requests = world
            .event_reader::<RouteAgentMessage>()
            .into_iter()
            .filter_map(|request| match &request.message {
                Message::User { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for content in requests {
            world.send_event(WebSocketMessageSend {
                target: WebSocketMessageTarget::Type("webui".into()),
                message: ServerMessage::Log {
                    record: LogRecordDto {
                        timestamp_millis: 1,
                        level: "INFO".into(),
                        target: "test".into(),
                        message: content,
                        fields: Vec::new(),
                        spans: Vec::new(),
                    },
                },
            });
        }
    });
    let address = start(&mut app);
    let client = thread::spawn(move || {
        client_runtime().block_on(async move {
            let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
                .await
                .unwrap();
            let registration =
                serde_json::to_string(&ClientMessage::register_connection("register-1", "webui"))
                    .unwrap();
            let message = serde_json::to_string(&ClientMessage::agent_message(
                "message-1",
                &WorkspaceReferenceDto::new("demo", "/tmp/demo"),
                None,
                "routed",
            ))
            .unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    registration.into(),
                ))
                .await
                .unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    message.into(),
                ))
                .await
                .unwrap();
            let response = loop {
                let response = socket.next().await.unwrap().unwrap();
                let tokio_tungstenite::tungstenite::Message::Text(text) = &response else {
                    continue;
                };
                let Ok(ServerMessage::Log { record }) = serde_json::from_str(text) else {
                    continue;
                };
                if record.message == "routed" {
                    break response;
                }
            };
            let _ = socket.close(None).await;
            response
        })
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    while !client.is_finished() {
        app.tick();
        assert!(Instant::now() < deadline, "API response timed out");
        thread::yield_now();
    }
    let response = client.join().unwrap();
    let tokio_tungstenite::tungstenite::Message::Text(response) = response else {
        panic!("expected a text response");
    };
    let response: ServerMessage = serde_json::from_str(&response).unwrap();
    assert!(matches!(
        response,
        ServerMessage::Log { record } if record.message == "routed"
    ));
}

#[test]
fn registration_creates_and_disconnect_removes_a_client_entity() {
    let mut app = build_app();
    let address = start(&mut app);
    let (release, released) = mpsc::channel::<()>();
    let client =
        thread::spawn(move || {
            let runtime = client_runtime();
            let (mut socket, _) = runtime
                .block_on(tokio_tungstenite::connect_async(format!(
                    "ws://{address}/ws"
                )))
                .unwrap();
            let registration = serde_json::to_string(
                &ClientMessage::register_connection_with_name("register-1", "webui", "console"),
            )
            .unwrap();
            runtime
                .block_on(socket.send(tokio_tungstenite::tungstenite::Message::Text(
                    registration.into(),
                )))
                .unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            runtime.block_on(socket.close(None)).unwrap();
        });

    let mut observed = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while observed.is_none() {
        app.tick();
        observed = first_client(&app);
        assert!(Instant::now() < deadline, "client entity was never created");
        thread::yield_now();
    }
    let (client_state, resource_id) = observed.unwrap();
    assert_eq!(client_state.client_type(), "webui");
    assert_eq!(client_state.name(), "console");
    assert_eq!(
        resource_id.to_string(),
        format!(
            "client:webui/console:{}",
            client_state.connection_id().get()
        )
    );
    assert_eq!(
        client_source(Some("webui"), Some("console"), client_state.connection_id()),
        resource_id.to_string()
    );
    assert_eq!(
        client_source(None, None, client_state.connection_id()),
        format!("unregistered:{}", client_state.connection_id().get())
    );

    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while first_client(&app).is_some() {
        app.tick();
        assert!(
            Instant::now() < deadline,
            "client entity was not removed after disconnect"
        );
        thread::yield_now();
    }
    client.join().unwrap();
}

fn registration_reply(client_type: &str, name: &str) -> ServerMessage {
    let client_type = client_type.to_owned();
    let name = name.to_owned();
    let mut app = build_app();
    let address = start(&mut app);
    let connection = thread::spawn(move || {
        client_runtime().block_on(async move {
            let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
                .await
                .unwrap();
            let registration = serde_json::to_string(
                &ClientMessage::register_connection_with_name("register-1", &client_type, &name),
            )
            .unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    registration.into(),
                ))
                .await
                .unwrap();
            let message = loop {
                let response = socket.next().await.unwrap().unwrap();
                let tokio_tungstenite::tungstenite::Message::Text(text) = &response else {
                    continue;
                };
                let Ok(message) = serde_json::from_str::<ServerMessage>(text) else {
                    continue;
                };
                if matches!(
                    message,
                    ServerMessage::ConnectionRegistered { .. }
                        | ServerMessage::ConnectionRegisterFailed { .. }
                ) {
                    break message;
                }
            };
            let _ = socket.close(None).await;
            message
        })
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while !connection.is_finished() {
        app.tick();
        assert!(Instant::now() < deadline, "registration reply timed out");
        thread::yield_now();
    }
    connection.join().unwrap()
}

#[test]
fn registration_replies_with_the_assigned_client_identity() {
    let reply = registration_reply("webui", "console");
    let ServerMessage::ConnectionRegistered { id, client } = reply else {
        panic!("expected connection.registered, got {reply:?}");
    };
    assert_eq!(id, "register-1");
    assert_eq!(client.client_type, "webui");
    assert_eq!(client.name, "console");
    assert!(
        client
            .resource_id
            .to_string()
            .starts_with("client:webui/console:"),
        "unexpected resource id {}",
        client.resource_id
    );
}

#[test]
fn registration_failure_replies_with_the_reason() {
    let reply = registration_reply("WebUI", "console");
    let ServerMessage::ConnectionRegisterFailed { id, error } = reply else {
        panic!("expected connection.register_failed, got {reply:?}");
    };
    assert_eq!(id, "register-1");
    assert_eq!(error, "client type is not a stable identifier");
}
