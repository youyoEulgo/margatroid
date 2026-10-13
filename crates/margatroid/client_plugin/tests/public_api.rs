use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use client_plugin::{
    client_source, registered_client, Client, ClientError, ClientPlugin, ClientRegistrationResult,
    RegisteredClient, WorldClientExt,
};
use core_plugin::App;
use resource_id_plugin::{
    LibraryComponent, ResourceComponentKind, ResourceID, ResourcePlugin, ResourceTypeRegistry,
    WorldResourceExt,
};
use server_plugin::{
    AppServerExt, RegisterConnection, ServerHandle, ServerPlugin, WebSocketConnectionId,
    WebSocketConnections,
};
use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
struct Harness {
    app: App,
    runtime: tokio::runtime::Runtime,
    address: SocketAddr,
}
impl Harness {
    fn new() -> Self {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ServerPlugin::bind("127.0.0.1:0"))
            .add_websocket_event_route("/ws")
            .add_plugin(ResourcePlugin)
            .add_plugin(ClientPlugin::default());
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
            assert!(Instant::now() < deadline, "server startup timed out");
            thread::yield_now();
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        Self {
            app,
            runtime,
            address,
        }
    }
    fn connect(&mut self) -> (Socket, WebSocketConnectionId) {
        let existing = self
            .app
            .world()
            .get_resource::<WebSocketConnections>()
            .unwrap()
            .get_all()
            .into_iter()
            .map(|sender| sender.connection_id())
            .collect::<Vec<_>>();
        let (socket, _) = self
            .runtime
            .block_on(tokio_tungstenite::connect_async(format!(
                "ws://{}/ws",
                self.address
            )))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let connection = loop {
            self.app.tick();
            if let Some(sender) = self
                .app
                .world()
                .get_resource::<WebSocketConnections>()
                .unwrap()
                .get_all()
                .into_iter()
                .find(|sender| !existing.contains(&sender.connection_id()))
            {
                break sender.connection_id();
            }
            assert!(
                Instant::now() < deadline,
                "connection registration timed out"
            );
            thread::yield_now();
        };
        (socket, connection)
    }
    fn register(
        &mut self,
        connection: WebSocketConnectionId,
        id: &str,
        kind: &str,
        name: Option<&str>,
    ) -> Result<RegisteredClient, ClientError> {
        self.app.world().send_event(RegisterConnection {
            id: id.into(),
            connection_id: connection,
            client_type: kind.into(),
            name: name.map(str::to_owned),
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.tick();
            if let Some(result) = self
                .app
                .world()
                .event_reader::<ClientRegistrationResult>()
                .into_iter()
                .find(|event| event.id == id)
            {
                return result.result.clone();
            }
            assert!(
                Instant::now() < deadline,
                "client registration reply timed out"
            );
            thread::yield_now();
        }
    }
    fn disconnect(&mut self, mut socket: Socket, connection: WebSocketConnectionId) {
        self.runtime.block_on(socket.close(None)).unwrap();
        drop(socket);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.tick();
            if self
                .app
                .world()
                .get_resource::<WebSocketConnections>()
                .unwrap()
                .get(connection)
                .is_none()
                && self.app.world().client_by_connection(connection).is_none()
            {
                break;
            }
            assert!(Instant::now() < deadline, "disconnect cleanup timed out");
            thread::yield_now();
        }
    }
}
#[test]
fn registration_creates_library_identity_before_success_and_disconnect_removes_it() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    assert!(registered_client(harness.app.world(), connection).is_none());
    let registered = harness
        .register(connection, "register", "webui", Some("console"))
        .unwrap();
    let world = harness.app.world();
    let client = world.get_component::<Client>(registered.client).unwrap();
    assert_eq!(client.resource_id(), &registered.resource_id);
    assert_eq!(
        client.resource_id.to_string(),
        format!("client:webui/console:{}", connection.get())
    );
    assert_eq!(client.client_type, "webui");
    assert_eq!(client.name, "console");
    assert_eq!(
        registered_client(world, connection),
        Some(registered.resource_id.clone())
    );
    assert_eq!(
        world
            .find_resource_entities(&registered.resource_id)
            .unwrap(),
        vec![registered.client]
    );
    assert_eq!(
        world
            .get_resource::<ResourceTypeRegistry>()
            .unwrap()
            .get("client")
            .unwrap()
            .kind,
        ResourceComponentKind::Library
    );
    harness.disconnect(socket, connection);
    assert!(!harness.app.world().is_alive(registered.client));
    assert!(registered_client(harness.app.world(), connection).is_none());
}
#[test]
fn identity_source_is_the_registered_component_not_connection_metadata() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    let registered = harness
        .register(connection, "register", "webui", Some("original"))
        .unwrap();
    let connections = harness
        .app
        .world()
        .get_resource::<WebSocketConnections>()
        .unwrap();
    connections.set_name(connection, "spoofed").unwrap();
    connections.set_connection_type(connection, "spoofed");
    assert_eq!(
        registered_client(harness.app.world(), connection),
        Some(registered.resource_id.clone())
    );
    assert_eq!(
        client_source(Some("webui"), Some("original"), connection),
        registered.resource_id.to_string()
    );
    assert_eq!(
        client_source(None, None, connection),
        format!("unregistered:{}", connection.get())
    );
    harness.disconnect(socket, connection);
}
#[test]
fn defaults_name_to_connection_id_and_rejects_reregistration() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    let registered = harness
        .register(connection, "register", "terminal", None)
        .unwrap();
    assert_eq!(registered.resource_id.name(), connection.get().to_string());
    assert_eq!(
        harness.register(connection, "second", "webui", Some("other")),
        Err(ClientError::DuplicateClient)
    );
    assert_eq!(harness.app.world().query_with::<Client>().result().len(), 1);
    harness.disconnect(socket, connection);
}
#[test]
fn invalid_registrations_reply_and_leave_no_client_or_metadata() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    for (id, kind, name, expected) in [
        ("", "webui", "valid", ClientError::InvalidRequest),
        ("type", "WebUI", "valid", ClientError::InvalidClientType),
        ("space", " webui ", "valid", ClientError::InvalidClientType),
        (
            "name",
            "webui",
            "../invalid",
            ClientError::InvalidResourceID,
        ),
    ] {
        assert_eq!(
            harness.register(connection, id, kind, Some(name)),
            Err(expected)
        );
    }
    assert!(harness
        .app
        .world()
        .query_with::<Client>()
        .result()
        .is_empty());
    let connections = harness
        .app
        .world()
        .get_resource::<WebSocketConnections>()
        .unwrap();
    assert!(connections.name(connection).is_none());
    assert!(connections.connection_type(connection).is_none());
    harness.disconnect(socket, connection);
}
#[test]
fn duplicate_name_cannot_create_half_registered_entity() {
    let mut harness = Harness::new();
    let (first, first_id) = harness.connect();
    let (second, second_id) = harness.connect();
    harness
        .register(first_id, "first", "webui", Some("console"))
        .unwrap();
    assert_eq!(
        harness.register(second_id, "second", "terminal", Some("console")),
        Err(ClientError::DuplicateName)
    );
    assert_eq!(harness.app.world().query_with::<Client>().result().len(), 1);
    assert!(harness
        .app
        .world()
        .get_resource::<WebSocketConnections>()
        .unwrap()
        .name(second_id)
        .is_none());
    harness.disconnect(second, second_id);
    harness.disconnect(first, first_id);
}
#[test]
fn missing_connection_registration_returns_correlated_error() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    harness.disconnect(socket, connection);
    assert_eq!(
        harness.register(connection, "missing", "webui", Some("console")),
        Err(ClientError::ConnectionMissing)
    );
    assert!(harness
        .app
        .world()
        .query_with::<Client>()
        .result()
        .is_empty());
}
#[test]
fn webui_sender_snapshot_is_registered_only_deduplicated_and_not_extended() {
    let mut harness = Harness::new();
    let (first, first_id) = harness.connect();
    let (second, second_id) = harness.connect();
    assert!(harness.app.world().client_webui_senders().is_empty());
    harness
        .register(first_id, "first", "webui", Some("first"))
        .unwrap();
    harness
        .register(second_id, "second", "terminal", Some("second"))
        .unwrap();
    let snapshot = harness.app.world().client_webui_senders();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].connection_id(), first_id);
    let (third, third_id) = harness.connect();
    harness
        .register(third_id, "third", "webui", Some("third"))
        .unwrap();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(harness.app.world().client_webui_senders().len(), 2);
    let duplicate = harness.app.world_mut().spawn();
    harness.app.world_mut().insert_component(
        duplicate,
        Client {
            resource_id: ResourceID::parse("client:webui/duplicate:1").unwrap(),
            connection_id: first_id,
            client_type: "webui".into(),
            name: "duplicate".into(),
        },
    );
    assert_eq!(harness.app.world().client_webui_senders().len(), 2);
    harness.disconnect(first, first_id);
    assert!(snapshot[0].is_closed());
    assert_eq!(harness.app.world().client_webui_senders().len(), 1);
    harness.disconnect(second, second_id);
    harness.disconnect(third, third_id);
}
#[test]
fn webui_snapshot_ignores_stale_client_and_absent_registry() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    harness.disconnect(socket, connection);
    let stale = harness.app.world_mut().spawn();
    harness.app.world_mut().insert_component(
        stale,
        Client {
            resource_id: ResourceID::parse("client:webui/stale:1").unwrap(),
            connection_id: connection,
            client_type: "webui".into(),
            name: "stale".into(),
        },
    );
    assert!(harness.app.world().client_webui_senders().is_empty());
    let mut world = core_plugin::World::new();
    let entity = world.spawn();
    world.insert_component(
        entity,
        harness
            .app
            .world()
            .get_component::<Client>(stale)
            .unwrap()
            .clone(),
    );
    assert!(world.client_webui_senders().is_empty());
}
