use std::collections::HashMap;
use std::error::Error as _;
use std::future::IntoFuture;
use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{AsyncRuntimeHandle, WorldAsyncExt};
use axum::body::{to_bytes, Body};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{Method, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, on, MethodFilter};
use axum::Router;
use core_plugin::{App, Plugin, World};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};

use crate::resource::{ErasedWebSocketClassifier, RouteRegistry, ServerHandle, WebSocketRoute};
use crate::websocket::{
    JsonWebSocketMessageClassifier, SharedConnectionState, SharedStreamState, WebSocketConnected,
    WebSocketConnectionId, WebSocketConnections, WebSocketDisconnected,
    WebSocketMessageClassification, WebSocketMessageClassifier, WebSocketMessageReceived,
    WebSocketProtocolError, WebSocketProtocolFailed, WebSocketSender, WebSocketStream,
    WebSocketStreamOpened, WebSocketStreamPhase, WebSocketStreamReceiver,
    WebSocketStreamReceiverHandle,
};
use crate::{
    HttpRequestReceived, HttpResponseSession, ServerError, ServerFailed, ServerOptions,
    ServerStarted, ServerStopped, WebSocketCloseReason, WebSocketStreamId,
};

#[derive(Clone, Debug, Default)]
pub struct ServerPlugin {
    options: ServerOptions,
}

impl ServerPlugin {
    pub fn bind(address: impl ToSocketAddrs) -> Self {
        let address = address
            .to_socket_addrs()
            .unwrap_or_else(|error| panic!("invalid server bind address: {error}"))
            .next()
            .expect("server bind address resolved to no socket addresses");
        Self::with_options(ServerOptions::bind(address))
    }

    pub fn with_options(options: ServerOptions) -> Self {
        Self { options }
    }

    pub fn with_body_limit(mut self, limit: usize) -> Self {
        self.options = self.options.with_body_limit(limit);
        self
    }

    pub fn with_response_start_timeout(mut self, timeout: Duration) -> Self {
        self.options = self.options.with_response_start_timeout(timeout);
        self
    }

    pub fn with_stream_buffer_capacity(mut self, capacity: usize) -> Self {
        self.options = self.options.with_stream_buffer_capacity(capacity);
        self
    }

    pub fn with_websocket_buffer_capacity(mut self, capacity: usize) -> Self {
        self.options = self.options.with_websocket_buffer_capacity(capacity);
        self
    }

    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.options = self.options.with_shutdown_timeout(timeout);
        self
    }
}

impl Plugin for ServerPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            ServerError::RuntimePluginMissing.panic();
        }
        if !app.world().contains_resource::<AsyncRuntimeHandle>() {
            ServerError::AsyncRuntimePluginMissing.panic();
        }
        if app.world().contains_resource::<RouteRegistry>()
            || app.world().contains_resource::<ServerHandle>()
            || app.world().contains_resource::<WebSocketConnections>()
        {
            ServerError::ServerPluginAlreadyInstalled.panic();
        }

        app.world_mut().insert_resource(RouteRegistry::new());
        app.world_mut().insert_resource(WebSocketConnections::new());
        app.world_mut().insert_resource(self.options);
        app.world_mut().insert_resource(ServerHandle::new());
        app.register_event::<HttpRequestReceived>()
            .register_event::<ServerStarted>()
            .register_event::<ServerFailed>()
            .register_event::<ServerStopped>()
            .register_event::<WebSocketConnected>()
            .register_event::<WebSocketMessageReceived>()
            .register_event::<WebSocketStreamOpened>()
            .register_event::<WebSocketDisconnected>()
            .register_event::<WebSocketProtocolFailed>()
            .add_system(RuntimePlugin::STARTUP, start_server);
    }
}

pub trait AppServerExt {
    fn add_http_routes(&mut self, router: Router) -> &mut Self;
    fn add_http_event_route(&mut self, method: Method, path: &str) -> &mut Self;
    fn add_websocket_event_route(&mut self, path: &str) -> &mut Self;
    fn add_websocket_event_route_with<C>(&mut self, path: &str, classifier: C) -> &mut Self
    where
        C: WebSocketMessageClassifier + Send + Sync + 'static;
}

impl AppServerExt for App {
    fn add_http_routes(&mut self, router: Router) -> &mut Self {
        self.world()
            .get_resource::<RouteRegistry>()
            .unwrap_or_else(|| ServerError::ServerPluginMissing.panic())
            .merge(router);
        self
    }

    fn add_http_event_route(&mut self, method: Method, path: &str) -> &mut Self {
        self.world()
            .get_resource::<RouteRegistry>()
            .unwrap_or_else(|| ServerError::ServerPluginMissing.panic())
            .add_event_route(method, path.into());
        self
    }

    fn add_websocket_event_route(&mut self, path: &str) -> &mut Self {
        self.add_websocket_event_route_with(path, JsonWebSocketMessageClassifier)
    }

    fn add_websocket_event_route_with<C>(&mut self, path: &str, classifier: C) -> &mut Self
    where
        C: WebSocketMessageClassifier + Send + Sync + 'static,
    {
        self.world()
            .get_resource::<RouteRegistry>()
            .unwrap_or_else(|| ServerError::ServerPluginMissing.panic())
            .add_websocket_route(path.into(), Arc::new(classifier));
        self
    }
}

#[derive(Clone)]
struct ServerBridgeState {
    event_sender: RuntimeEventSender,
    websocket_connections: WebSocketConnections,
    next_request_id: Arc<AtomicU64>,
    next_websocket_id: Arc<AtomicU64>,
    body_limit: usize,
    response_start_timeout: Duration,
    stream_buffer_capacity: usize,
    websocket_buffer_capacity: usize,
}

#[derive(Clone)]
struct WebSocketRouteState {
    bridge: ServerBridgeState,
    classifier: ErasedWebSocketClassifier,
}

fn start_server(world: &mut World) {
    let options = world
        .get_resource::<ServerOptions>()
        .expect("ServerOptions should be registered")
        .clone();
    let handle = world
        .get_resource::<ServerHandle>()
        .expect("ServerHandle should be registered")
        .clone();
    let bridge = ServerBridgeState {
        event_sender: world.event_sender(),
        websocket_connections: world
            .get_resource::<WebSocketConnections>()
            .expect("WebSocketConnections should be registered")
            .clone(),
        next_request_id: Arc::new(AtomicU64::new(1)),
        next_websocket_id: Arc::new(AtomicU64::new(1)),
        body_limit: options.body_limit,
        response_start_timeout: options.response_start_timeout,
        stream_buffer_capacity: options.stream_buffer_capacity,
        websocket_buffer_capacity: options.websocket_buffer_capacity,
    };
    let (router, event_routes, websocket_routes) = world
        .get_resource::<RouteRegistry>()
        .expect("RouteRegistry should be registered")
        .freeze();
    let router = build_router(router, event_routes, websocket_routes, bridge.clone());
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    handle.set_shutdown_sender(shutdown_sender);
    world.spawn_async_service(run_server(
        options,
        router,
        handle,
        bridge.event_sender,
        shutdown_receiver,
    ));
}

fn build_router(
    mut router: Router,
    event_routes: Vec<crate::resource::EventRoute>,
    websocket_routes: Vec<WebSocketRoute>,
    bridge: ServerBridgeState,
) -> Router {
    for route in event_routes {
        let method_filter = MethodFilter::try_from(route.method.clone()).unwrap_or_else(|_| {
            ServerError::UnsupportedMethod {
                method: route.method,
            }
            .panic()
        });
        let event_router = Router::new()
            .route(&route.path, on(method_filter, handle_event_request))
            .with_state(bridge.clone());
        router = router.merge(event_router);
    }
    for route in websocket_routes {
        let state = WebSocketRouteState {
            bridge: bridge.clone(),
            classifier: route.classifier,
        };
        let websocket_router = Router::new()
            .route(&route.path, get(handle_websocket_upgrade))
            .with_state(state);
        router = router.merge(websocket_router);
    }
    router
}

async fn run_server(
    options: ServerOptions,
    router: Router,
    handle: ServerHandle,
    event_sender: RuntimeEventSender,
    shutdown_receiver: oneshot::Receiver<()>,
) {
    let listener = match tokio::net::TcpListener::bind(options.bind_address).await {
        Ok(listener) => listener,
        Err(error) => {
            event_sender.send_event(ServerFailed {
                message: error.to_string(),
            });
            handle.mark_stopped();
            return;
        }
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => {
            event_sender.send_event(ServerFailed {
                message: error.to_string(),
            });
            handle.mark_stopped();
            return;
        }
    };
    handle.set_local_address(address);
    event_sender.send_event(ServerStarted { address });

    let (graceful_sender, graceful_receiver) = oneshot::channel();
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = graceful_receiver.await;
        })
        .into_future();
    tokio::pin!(server);

    tokio::select! {
        result = &mut server => {
            if let Err(error) = result {
                event_sender.send_event(ServerFailed { message: error.to_string() });
            }
        }
        _ = shutdown_receiver => {
            let _ = graceful_sender.send(());
            match tokio::time::timeout(options.shutdown_timeout, &mut server).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    event_sender.send_event(ServerFailed { message: error.to_string() });
                }
                Err(_) => {
                    tracing::warn!("server exceeded graceful shutdown timeout");
                }
            }
        }
    }
    handle.mark_stopped();
    event_sender.send_event(ServerStopped);
}

async fn handle_event_request(
    State(state): State<ServerBridgeState>,
    request: Request,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, state.body_limit).await {
        Ok(body) => body,
        Err(error) if is_body_limit_error(&error) => {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let (response_sender, response_receiver) = oneshot::channel();
    let response = HttpResponseSession::new(response_sender, state.stream_buffer_capacity);
    let request_id = state.next_request_id.fetch_add(1, Ordering::Relaxed);
    state.event_sender.send_event(HttpRequestReceived::new(
        request_id,
        parts.method,
        parts.uri,
        parts.headers,
        body,
        response,
    ));

    match tokio::time::timeout(state.response_start_timeout, response_receiver).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
    }
}

fn is_body_limit_error(error: &axum::Error) -> bool {
    let mut source = error.source();
    while let Some(current) = source {
        if current.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        source = current.source();
    }
    false
}

async fn handle_websocket_upgrade(
    State(state): State<WebSocketRouteState>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let connection_id = WebSocketConnectionId::new(
        state
            .bridge
            .next_websocket_id
            .fetch_add(1, Ordering::Relaxed),
    );
    upgrade.on_upgrade(move |socket| run_websocket_connection(state, connection_id, socket))
}

async fn run_websocket_connection(
    route: WebSocketRouteState,
    connection_id: WebSocketConnectionId,
    socket: WebSocket,
) {
    let (mut writer, mut reader) = socket.split();
    let (message_sender, mut message_receiver) =
        mpsc::channel(route.bridge.websocket_buffer_capacity);
    let connection_state = Arc::new(SharedConnectionState::open());
    let sender = WebSocketSender::new(
        connection_id,
        message_sender.clone(),
        Arc::clone(&connection_state),
    );
    route.bridge.websocket_connections.insert(sender.clone());
    route
        .bridge
        .event_sender
        .send_event(WebSocketConnected { connection_id });

    enum CompletedLoop {
        Read(Option<WebSocketCloseReason>),
        Write(Option<WebSocketCloseReason>),
    }

    let completed = {
        let read = read_websocket_messages(
            &route.bridge.event_sender,
            route.classifier,
            connection_id,
            sender.clone(),
            route.bridge.websocket_buffer_capacity,
            &mut reader,
        );
        let write = write_websocket_messages(&mut writer, &mut message_receiver);
        tokio::pin!(read);
        tokio::pin!(write);
        tokio::select! {
            reason = &mut read => CompletedLoop::Read(reason),
            reason = &mut write => CompletedLoop::Write(reason),
        }
    };
    let reason = match completed {
        CompletedLoop::Read(reason) => {
            let close = reason
                .clone()
                .unwrap_or_else(|| WebSocketCloseReason::new(1000, "connection closed"));
            let _ = writer
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: close.code(),
                    reason: close.reason().to_owned().into(),
                })))
                .await;
            reason
        }
        CompletedLoop::Write(reason) => reason,
    };
    connection_state.close();
    route.bridge.websocket_connections.remove(connection_id);
    route.bridge.event_sender.send_event(WebSocketDisconnected {
        connection_id,
        reason,
    });
}

async fn write_websocket_messages(
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    messages: &mut mpsc::Receiver<Message>,
) -> Option<WebSocketCloseReason> {
    while let Some(message) = messages.recv().await {
        let close_reason = match &message {
            Message::Close(frame) => frame.clone().map(WebSocketCloseReason::from_frame),
            _ => None,
        };
        if writer.send(message).await.is_err() {
            return close_reason;
        }
        if close_reason.is_some() {
            return close_reason;
        }
    }
    None
}

async fn read_websocket_messages(
    event_sender: &RuntimeEventSender,
    classifier: ErasedWebSocketClassifier,
    connection_id: WebSocketConnectionId,
    sender: WebSocketSender,
    stream_buffer_capacity: usize,
    reader: &mut futures_util::stream::SplitStream<WebSocket>,
) -> Option<WebSocketCloseReason> {
    let mut streams = HashMap::<WebSocketStreamId, WebSocketStream>::new();
    let close_reason = loop {
        match reader.next().await {
            Some(Ok(Message::Ping(body))) => {
                if sender.send(Message::Pong(body)).await.is_err() {
                    break None;
                }
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(frame))) => {
                break frame.map(WebSocketCloseReason::from_frame);
            }
            Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                route_websocket_message(
                    event_sender,
                    classifier.as_ref(),
                    connection_id,
                    stream_buffer_capacity,
                    &mut streams,
                    message,
                )
                .await;
            }
            Some(Err(_)) | None => break None,
        }
    };
    for stream in streams.values() {
        stream.state.disconnect();
    }
    streams.clear();
    close_reason
}

async fn route_websocket_message(
    event_sender: &RuntimeEventSender,
    classifier: &dyn WebSocketMessageClassifier,
    connection_id: WebSocketConnectionId,
    stream_buffer_capacity: usize,
    streams: &mut HashMap<WebSocketStreamId, WebSocketStream>,
    message: Message,
) {
    let classification = match classifier.classify(message) {
        Ok(classification) => classification,
        Err(error) => {
            event_sender.send_event(WebSocketProtocolFailed {
                connection_id,
                error,
            });
            return;
        }
    };
    let WebSocketMessageClassification::Stream {
        stream_id,
        phase,
        message,
    } = classification
    else {
        let WebSocketMessageClassification::Ordinary { message } = classification else {
            unreachable!();
        };
        event_sender.send_event(WebSocketMessageReceived {
            connection_id,
            message,
        });
        return;
    };

    match phase {
        WebSocketStreamPhase::Start => {
            if streams.contains_key(&stream_id) {
                event_sender.send_event(WebSocketProtocolFailed {
                    connection_id,
                    error: WebSocketProtocolError::DuplicateStream {
                        connection_id,
                        stream_id,
                    },
                });
                return;
            }
            let (sender, receiver) = mpsc::channel(stream_buffer_capacity);
            let stream_state = Arc::new(SharedStreamState::open());
            if sender.send(message).await.is_err() {
                return;
            }
            let receiver = WebSocketStreamReceiver::new(receiver, Arc::clone(&stream_state));
            streams.insert(
                stream_id.clone(),
                WebSocketStream {
                    sender,
                    state: stream_state,
                },
            );
            event_sender.send_event(WebSocketStreamOpened {
                connection_id,
                stream_id,
                receiver: WebSocketStreamReceiverHandle::new(receiver),
            });
        }
        WebSocketStreamPhase::Chunk => {
            let Some(stream) = streams.get(&stream_id) else {
                event_sender.send_event(WebSocketProtocolFailed {
                    connection_id,
                    error: WebSocketProtocolError::UnknownStream {
                        connection_id,
                        stream_id,
                    },
                });
                return;
            };
            if stream.sender.send(message).await.is_err() {
                if let Some(stream) = streams.remove(&stream_id) {
                    stream.state.abort();
                }
            }
        }
        WebSocketStreamPhase::End | WebSocketStreamPhase::Abort => {
            let Some(stream) = streams.remove(&stream_id) else {
                event_sender.send_event(WebSocketProtocolFailed {
                    connection_id,
                    error: WebSocketProtocolError::UnknownStream {
                        connection_id,
                        stream_id,
                    },
                });
                return;
            };
            if stream.sender.send(message).await.is_err() {
                stream.state.abort();
                return;
            }
            match phase {
                WebSocketStreamPhase::End => stream.state.finish(),
                WebSocketStreamPhase::Abort => stream.state.abort(),
                WebSocketStreamPhase::Start | WebSocketStreamPhase::Chunk => unreachable!(),
            }
        }
    }
}
