use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use client_plugin::ClientPlugin;
use config_plugin::{ConfigPlugin, LogLevel, MargatroidConfig, WebSocketMessageTarget};
use core_plugin::App;
use dto_plugin::DtoPlugin;
use futures_util::{SinkExt, StreamExt};
use log_plugin::LogPlugin;
use margatroid_protocol::{
    AgentMessageDto, ClientMessage, IntoDomain, ServerMessage, WorkspaceReferenceDto,
};
use margatroid_types::{agent_components::Agent, workspace_components::Workspace};
use resource_id_plugin::ResourcePlugin;
use serde_json::json;
use server_plugin::{ServerHandle, ServerPlugin};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
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
        .add_plugin(ClientPlugin::default())
        .add_plugin(DtoPlugin::default());
    app
}

fn start(app: &mut App) -> SocketAddr {
    app.tick();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(address) = app
            .world()
            .get_resource::<ServerHandle>()
            .and_then(ServerHandle::local_address)
        {
            return address;
        }
        assert!(Instant::now() < deadline, "DTO WebSocket startup timed out");
        thread::yield_now();
    }
}

async fn reply(socket: &mut Socket, id: &str) -> ServerMessage {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = socket
                .next()
                .await
                .expect("WebSocket ended before reply")
                .unwrap();
            if let Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["id"] == id {
                    return serde_json::from_value(value).unwrap();
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no correlated DTO response for {id}"))
}

async fn missing_workspace_image(socket: &mut Socket, id: &str, image_bytes: usize) {
    let mut data = "iVBORw0KGgoA".to_owned();
    data.push_str(&"A".repeat(image_bytes));
    let user = json!({"type":"user", "content":"describe this image", "images":[{
        "media_type":"image/png", "data":data
    }]});
    let dto: AgentMessageDto = serde_json::from_value(user.clone()).unwrap();
    let _: margatroid_types::AgentMessage = dto.into_domain(()).unwrap();
    let request = ClientMessage::mcl_command(
        id,
        &WorkspaceReferenceDto::new("absent-image-workspace", "/not-created-by-this-test"),
        None,
        "INJECT ? TO context",
        vec![json!([user])],
    );
    let text = serde_json::to_string(&request).unwrap();
    if image_bytes > 1024 * 1024 {
        assert!(text.len() > 1024 * 1024);
        assert!(text.len() < 32 * 1024 * 1024);
    } else {
        assert!(text.len() < 1024 * 1024);
    }
    socket.send(Message::Text(text.into())).await.unwrap();
    match reply(socket, id).await {
        ServerMessage::RequestFailed {
            id: response_id,
            error,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(error.kind, "WorkspaceNotFound");
            assert!(!error.message.contains("iVBORw0KGgoA"));
        }
        other => panic!("expected correlated workspace routing failure, got {other:?}"),
    }
}

#[test]
fn real_websocket_routes_image_requests_above_one_mib_and_keeps_connection_usable() {
    let mut app = app();
    let address = start(&mut app);
    let client = thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let (mut socket, _) =
                    tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
                        .await
                        .unwrap();
                let register = ClientMessage::register_connection_with_name(
                    "register-images",
                    "webui",
                    "image-regression",
                );
                socket
                    .send(Message::Text(
                        serde_json::to_string(&register).unwrap().into(),
                    ))
                    .await
                    .unwrap();
                assert!(matches!(
                    reply(&mut socket, "register-images").await,
                    ServerMessage::ConnectionRegistered { .. }
                ));

                missing_workspace_image(&mut socket, "small-before", 128).await;
                missing_workspace_image(&mut socket, "image-over-one-mib", 4 * 400_000).await;
                missing_workspace_image(&mut socket, "image-over-four-mib", 4 * 1_100_000).await;
                missing_workspace_image(&mut socket, "small-after", 128).await;
                let _ = socket.close(None).await;
            });
    });
    let deadline = Instant::now() + Duration::from_secs(25);
    while !client.is_finished() {
        app.tick();
        assert!(
            Instant::now() < deadline,
            "DTO image regression client timed out"
        );
        thread::yield_now();
    }
    client.join().unwrap();
    assert!(app.world().query_with::<Workspace>().result().is_empty());
    assert!(app.world().query_with::<Agent>().result().is_empty());
}
