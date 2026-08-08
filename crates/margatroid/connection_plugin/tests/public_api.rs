use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use api_plugin::{AgentMessageRequested, ApiPlugin, WebSocketMessageSend, WebSocketMessageTarget};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use connection_plugin::ConnectionPlugin;
use core_plugin::{App, World};
use futures_util::{SinkExt, StreamExt};
use margatroid_protocol::{ClientRequest, LogRecordDto, ServerEvent, WorkspaceRefDto};
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
        .add_plugin(ServerPlugin::bind("127.0.0.1:0"))
        .add_plugin(ApiPlugin::default())
        .add_plugin(ConnectionPlugin::default())
        .add_system(RuntimePlugin::UPDATE, |world: &mut World| {
            let requests = world
                .event_reader::<AgentMessageRequested>()
                .into_iter()
                .map(|request| request.content.clone())
                .collect::<Vec<_>>();
            for content in requests {
                world.send_event(WebSocketMessageSend {
                    target: WebSocketMessageTarget::Type("webui".into()),
                    message: ServerEvent::Log {
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
                serde_json::to_string(&ClientRequest::register_connection("webui")).unwrap();
            let message = serde_json::to_string(&ClientRequest::agent_message(
                "message-1",
                &WorkspaceRefDto::new("demo", "/tmp/demo"),
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
            let response = socket.next().await.unwrap().unwrap();
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
    let response: ServerEvent = serde_json::from_str(&response).unwrap();
    assert!(matches!(
        response,
        ServerEvent::Log { record } if record.message == "routed"
    ));
}
