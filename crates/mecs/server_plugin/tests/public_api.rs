use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::{AsyncRuntimePlugin, AsyncTaskError, WorldAsyncExt};
use axum::body::Bytes;
use axum::extract::ws::Message as AxumMessage;
use axum::http::{Method, Response};
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, World};
use futures_util::{SinkExt, StreamExt};
use server_plugin::{
    AppServerExt, HttpRequestReceived, HttpResponseHead, ServerHandle, ServerPlugin,
    WebSocketConnected, WebSocketConnections, WebSocketDisconnected, WebSocketMessageReceived,
    WebSocketStreamOpened,
};

#[derive(Debug)]
struct TestAsyncError;

impl From<AsyncTaskError> for TestAsyncError {
    fn from(_error: AsyncTaskError) -> Self {
        Self
    }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ServerPlugin::bind("127.0.0.1:0"));
    app
}

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
fn delegated_http_request_is_processed_by_a_system() {
    let mut app = app();
    app.add_http_event_route(Method::POST, "/echo").add_system(
        RuntimePlugin::UPDATE,
        |world: &mut World| {
            for request in world.event_reader::<HttpRequestReceived>() {
                request
                    .respond(Response::new(request.body().clone()))
                    .unwrap();
            }
        },
    );
    let address = start(&mut app);
    let request = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(
                b"POST /echo HTTP/1.1\r\nhost: localhost\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
            )
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    while !request.is_finished() {
        app.tick();
        assert!(Instant::now() < deadline, "HTTP request timed out");
        thread::yield_now();
    }
    let response = request.join().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with("hello"), "{response}");
}

#[test]
fn delegated_http_response_can_stream_from_an_async_closure() {
    let mut app = app();
    app.add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_http_event_route(Method::POST, "/stream")
        .add_system(RuntimePlugin::UPDATE, |world: &mut World| {
            let responses = world
                .event_reader::<HttpRequestReceived>()
                .into_iter()
                .map(|request| {
                    request.start_stream(HttpResponseHead::default()).unwrap();
                    request.response_session()
                })
                .collect::<Vec<_>>();
            for response in responses {
                world.send_async_closure(RuntimePlugin::PRE_UPDATE, move || async move {
                    response
                        .send_chunk(Bytes::from_static(b"hello "))
                        .await
                        .map_err(|_| TestAsyncError)?;
                    response
                        .send_chunk(Bytes::from_static(b"world"))
                        .await
                        .map_err(|_| TestAsyncError)?;
                    response.finish().map_err(|_| TestAsyncError)?;
                    Ok::<(), TestAsyncError>(())
                });
            }
        });
    let address = start(&mut app);
    let request = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(
                b"POST /stream HTTP/1.1\r\nhost: localhost\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    while !request.is_finished() {
        app.tick();
        assert!(
            Instant::now() < deadline,
            "streaming HTTP request timed out"
        );
        thread::yield_now();
    }
    let response = request.join().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("hello "), "{response}");
    assert!(response.ends_with("world\r\n0\r\n\r\n"), "{response}");
}

#[test]
fn websocket_message_is_delegated_and_can_be_answered() {
    let mut app = app();
    app.add_websocket_event_route("/ws")
        .add_system(RuntimePlugin::UPDATE, |world: &mut World| {
            let connected = world
                .event_reader::<WebSocketConnected>()
                .into_iter()
                .map(|event| event.connection_id)
                .collect::<Vec<_>>();
            let disconnected = world
                .event_reader::<WebSocketDisconnected>()
                .into_iter()
                .map(|event| event.connection_id)
                .collect::<Vec<_>>();
            let messages = world
                .event_reader::<WebSocketMessageReceived>()
                .into_iter()
                .filter_map(|event| {
                    let AxumMessage::Text(message) = &event.message else {
                        return None;
                    };
                    Some((event.connection_id, message.clone()))
                })
                .collect::<Vec<_>>();
            let connections = world.get_resource::<WebSocketConnections>().unwrap();
            for connection_id in connected {
                assert!(connections.get(connection_id).is_some());
            }
            for connection_id in disconnected {
                assert!(connections.get(connection_id).is_none());
            }
            for (connection_id, message) in messages {
                connections.set_name(connection_id, "echo-client").unwrap();
                connections
                    .get_by_name("echo-client")
                    .unwrap()
                    .try_send(AxumMessage::Text(format!("echo:{message}").into()))
                    .unwrap();
            }
        });
    let address = start(&mut app);
    let client = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            use tokio_tungstenite::tungstenite::Message;

            let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
                .await
                .unwrap();
            socket.send(Message::Text("hello".into())).await.unwrap();
            let response = socket.next().await.unwrap().unwrap();
            let _ = socket.close(None).await;
            response
        })
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    while !client.is_finished() {
        app.tick();
        assert!(Instant::now() < deadline, "WebSocket request timed out");
        thread::yield_now();
    }
    assert_eq!(
        client.join().unwrap(),
        tokio_tungstenite::tungstenite::Message::Text("echo:hello".into())
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        app.tick();
        let connections = app.world().get_resource::<WebSocketConnections>().unwrap();
        if connections.get_by_name("echo-client").is_none() && connections.unnamed().is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "WebSocket connection was not removed"
        );
        thread::yield_now();
    }
}

#[test]
fn websocket_stream_is_accumulated_asynchronously_and_returns_to_ecs() {
    let mut app = app();
    app.add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_websocket_event_route("/ws")
        .add_system(RuntimePlugin::UPDATE, |world: &mut World| {
            for event in world.event_reader::<WebSocketStreamOpened>() {
                let Some(mut receiver) = event.receiver.take() else {
                    continue;
                };
                world.send_async_closure(RuntimePlugin::PRE_UPDATE, move || async move {
                    let mut messages = Vec::new();
                    while let Some(message) = receiver.recv().await {
                        let message = message.map_err(|_| TestAsyncError)?;
                        if let AxumMessage::Text(text) = message {
                            messages.push(text.to_string());
                        }
                    }
                    Ok::<_, TestAsyncError>(messages)
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
            use tokio_tungstenite::tungstenite::Message;

            let (mut socket, _) =
                tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
                    .await
                    .unwrap();
            for phase in ["start", "chunk", "end"] {
                socket
                    .send(Message::Text(
                        format!(
                            r#"{{"mecs_stream":{{"id":"prompt-1","phase":"{phase}"}},"payload":"{phase}"}}"#
                        )
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = socket.close(None).await;
        });
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    let messages = loop {
        app.tick();
        if let Some(Ok(messages)) = app
            .world()
            .event_reader::<Result<Vec<String>, TestAsyncError>>()
            .into_iter()
            .next()
        {
            break messages.clone();
        }
        assert!(Instant::now() < deadline, "WebSocket stream timed out");
        thread::yield_now();
    };
    client.join().unwrap();
    assert_eq!(messages.len(), 3);
    assert!(messages[0].contains(r#""phase":"start""#));
    assert!(messages[1].contains(r#""phase":"chunk""#));
    assert!(messages[2].contains(r#""phase":"end""#));
}

#[test]
fn native_axum_routes_remain_available() {
    use axum::routing::get;
    use axum::Router;

    let mut app = app();
    app.add_http_routes(
        Router::new().route("/health", get(|| async { Bytes::from_static(b"ok") })),
    );
    let address = start(&mut app);
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
}
