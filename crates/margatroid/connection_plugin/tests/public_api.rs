use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use config_plugin::{ConfigPlugin, MargatroidConfig, WebSocketMessageTarget};
use connection_plugin::ConnectionPlugin;
use core_plugin::{App, World};
use dto_plugin::{DtoPlugin, WebSocketMessageSend};
use futures_util::{SinkExt, StreamExt};
use log_plugin::LogPlugin;
use margatroid_protocol::{ClientMessage, LogRecordDto, ServerMessage, WorkspaceReferenceDto};
use margatroid_types::{Message, RouteAgentMessage};
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

#[test]
fn registered_client_receives_messages_targeted_by_type() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(LogPlugin::default().without_console().with_stream(8))
        .add_plugin(ServerPlugin::bind("127.0.0.1:0"))
        .add_plugin(ConfigPlugin::new(
            MargatroidConfig::new(
                vec![WebSocketMessageTarget::Broadcast],
                vec![WebSocketMessageTarget::Broadcast],
                vec![WebSocketMessageTarget::Broadcast],
                vec![WebSocketMessageTarget::Broadcast],
            )
            .unwrap(),
        ))
        .add_plugin(DtoPlugin::default())
        .add_plugin(ConnectionPlugin::default())
        .add_system(RuntimePlugin::UPDATE, |world: &mut World| {
            let requests = world
                .event_reader::<RouteAgentMessage>()
                .into_iter()
                .filter_map(|request| match &request.message {
                    Message::User { content } => Some(content.clone()),
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
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
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
