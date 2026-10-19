use std::thread;
use std::time::{Duration, Instant};

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use config_plugin::{ConfigPlugin, LogLevel, MargatroidConfig, WebSocketMessageTarget};
use core_plugin::App;
use dto_plugin::DtoPlugin;
use futures_util::{SinkExt, StreamExt};
use log_plugin::LogPlugin;
use serde_json::json;
use server_plugin::{ServerHandle, ServerPlugin};
use tokio_tungstenite::tungstenite::{protocol::frame::coding::CloseCode, Message};

#[test]
fn real_websocket_invalid_requests_fail_promptly_without_exposing_payloads() {
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
            )
            .unwrap(),
        ))
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
        assert!(Instant::now() < deadline, "DTO WebSocket startup timed out");
        thread::yield_now();
    };
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
                let secret = "private-image-payload-not-for-errors";
                for kind in ["mcl.command", "history.query", "agent.turn.abort"] {
                    for explicit_null in [false, true] {
                        let mut message = json!({"workspace":{}, "private":secret});
                        if explicit_null {
                            message["agent"] = serde_json::Value::Null;
                        }
                        socket
                            .send(Message::Text(
                                json!({"type":kind,"id":"missing-agent","message":message})
                                    .to_string()
                                    .into(),
                            ))
                            .await
                            .unwrap();
                        tokio::time::timeout(Duration::from_secs(5), async {
                            loop {
                                if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                                    assert!(!text.contains(secret));
                                    let value: serde_json::Value =
                                        serde_json::from_str(&text).unwrap();
                                    if value["id"] == "missing-agent" {
                                        assert_eq!(value["type"], "request.failed");
                                        assert_eq!(value["error"]["kind"], "InvalidRequest");
                                        assert!(value["error"]["message"]
                                            .as_str()
                                            .unwrap()
                                            .contains("message.agent"));
                                        break;
                                    }
                                }
                            }
                        })
                        .await
                        .expect("missing/null agent must receive an actionable failure");
                    }
                }
                socket
                    .send(Message::Text(
                        json!({"type":secret,"id":"malformed-correlated"})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        let message = socket.next().await.unwrap().unwrap();
                        if let Message::Text(text) = message {
                            assert!(!text.contains(secret), "response leaked invalid payload");
                            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                            if value["id"] == "malformed-correlated" {
                                assert_eq!(value["type"], "request.failed");
                                assert_eq!(value["error"]["kind"], "InvalidRequest");
                                break;
                            }
                        }
                    }
                })
                .await
                .expect("malformed request with ID must receive a correlated failure");
                socket
                    .send(Message::Text(format!("invalid-json:{secret}").into()))
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        match socket.next().await.unwrap().unwrap() {
                            Message::Close(Some(frame)) => {
                                assert_eq!(frame.code, CloseCode::Invalid);
                                assert!(!frame.reason.contains(secret));
                                break;
                            }
                            Message::Text(text) => {
                                assert!(!text.contains(secret), "log leaked invalid payload");
                            }
                            other => {
                                panic!("expected explicit invalid-request close, got {other:?}")
                            }
                        }
                    }
                })
                .await
                .expect("malformed request without ID must close promptly");
            });
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while !client.is_finished() {
        app.tick();
        assert!(Instant::now() < deadline, "invalid DTO requests timed out");
        thread::yield_now();
    }
    client.join().unwrap();
}
