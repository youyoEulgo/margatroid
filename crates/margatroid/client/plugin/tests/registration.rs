use address::{Address, WorldAddressExt};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use client_plugin::{
    Client, ClientError, ClientPlugin, ClientRegistrationResult, RegisteredClient, WorldClientExt,
};
use core_plugin::App;
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
fn registration_has_an_independent_address_and_disconnect_destroys_both() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    let registered = harness
        .register(connection, "register", "webui", Some("console"))
        .unwrap();
    let world = harness.app.world();
    let client = world.get_component::<Client>(registered.client).unwrap();
    assert_eq!(client.name, "console");
    let address = format!("client:webui/console:{}", connection.get());
    assert_eq!(
        world.entity_by_address(&address).unwrap(),
        registered.client
    );
    assert_eq!(
        world.client_by_connection(connection).unwrap().0,
        registered.client
    );
    harness.disconnect(socket, connection);
    assert!(!harness.app.world().is_alive(registered.client));
    assert!(!harness.app.world().address_exists(&address).unwrap());
}

#[test]
fn failed_registration_never_updates_metadata_or_spawns_a_client() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    for (id, kind, name, error) in [
        ("", "webui", "valid", ClientError::InvalidRequest),
        ("type", "WebUI", "valid", ClientError::InvalidClientType),
        ("name", "webui", "../invalid", ClientError::InvalidName),
        ("empty", "webui", "", ClientError::InvalidName),
    ] {
        assert_eq!(
            harness.register(connection, id, kind, Some(name)),
            Err(error)
        );
        assert!(harness
            .app
            .world()
            .query_with::<Client>()
            .result()
            .is_empty());
    }
    harness.disconnect(socket, connection);
}

#[test]
fn occupied_address_is_rejected_without_overwriting_connection_or_owner() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    let blocker = harness.app.world_mut().spawn();
    let value = format!("client:webui/console:{}", connection.get());
    harness
        .app
        .world_mut()
        .insert_component(blocker, Address::new(&value).unwrap());
    assert_eq!(
        harness.register(connection, "register", "webui", Some("console")),
        Err(ClientError::AddressConflict)
    );
    assert_eq!(
        harness.app.world().entity_by_address(&value).unwrap(),
        blocker
    );
    assert!(harness
        .app
        .world()
        .client_by_connection(connection)
        .is_none());
    harness.disconnect(socket, connection);
}

#[test]
fn source_identity_uses_component_instead_of_mutable_connection_labels() {
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
    let world = harness.app.world();
    let (entity, client) = world.client_by_connection(connection).unwrap();
    assert_eq!(entity, registered.client);
    assert_eq!(client.name, "original");
    assert_eq!(client.client_type, "webui");
    assert_eq!(world.client_webui_senders().len(), 1);
    harness.disconnect(socket, connection);
}

#[test]
fn default_name_and_duplicate_registration_use_current_connection() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    let registered = harness
        .register(connection, "register", "webui", None)
        .unwrap();
    assert_eq!(
        harness
            .app
            .world()
            .get_component::<Client>(registered.client)
            .unwrap()
            .name,
        connection.get().to_string()
    );
    assert_eq!(
        harness.register(connection, "again", "webui", Some("other")),
        Err(ClientError::DuplicateClient)
    );
    harness.disconnect(socket, connection);
}
