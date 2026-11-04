use address::{Address, WorldAddressExt};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use client_plugin::{
    Client, ClientDisconnected, ClientError, ClientFrame, ClientPlugin, ClientRegistrationResult,
    ClientSender, ConnectionId, RegisterClient, RegisteredClient, WorldClientExt,
};
use core_plugin::App;
use std::collections::BTreeMap;
struct Harness {
    app: App,
    next: u64,
    senders: BTreeMap<ConnectionId, ClientSender>,
}
impl Harness {
    fn new() -> Self {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClientPlugin::default());
        Self {
            app,
            next: 0,
            senders: BTreeMap::new(),
        }
    }
    fn connect(&mut self) -> (tokio::sync::mpsc::Receiver<ClientFrame>, ConnectionId) {
        self.next += 1;
        let id = ConnectionId::new(self.next);
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        self.senders.insert(id, ClientSender::new(tx));
        (rx, id)
    }
    fn register(
        &mut self,
        connection: ConnectionId,
        id: &str,
        kind: &str,
        name: Option<&str>,
    ) -> Result<RegisteredClient, ClientError> {
        self.app.world().send_event(RegisterClient {
            id: id.into(),
            connection_id: connection,
            client_type: kind.into(),
            name: name.map(str::to_owned),
            sender: self.senders[&connection].clone(),
        });
        self.app.tick();
        self.app.tick();
        self.app
            .world()
            .event_reader::<ClientRegistrationResult>()
            .into_iter()
            .find(|event| event.id == id)
            .unwrap()
            .result
            .clone()
    }
    fn disconnect(
        &mut self,
        receiver: tokio::sync::mpsc::Receiver<ClientFrame>,
        connection: ConnectionId,
    ) {
        drop(receiver);
        self.app.world().send_event(ClientDisconnected {
            connection_id: connection,
        });
        self.app.tick();
        self.app.tick();
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
fn failed_registration_never_spawns_a_client() {
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
fn occupied_address_is_rejected_without_overwriting_owner() {
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
fn source_identity_and_sender_snapshot_use_registered_components() {
    let mut harness = Harness::new();
    let (socket, connection) = harness.connect();
    let registered = harness
        .register(connection, "register", "webui", Some("original"))
        .unwrap();
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

#[test]
fn closed_receiver_rejects_registration_and_sender_reports_capacity() {
    let mut harness = Harness::new();
    let (mut receiver, id) = harness.connect();
    let sender = harness.senders[&id].clone();
    for _ in 0..8 {
        sender.try_send(ClientFrame::Text("test".into())).unwrap();
    }
    assert_eq!(
        sender.try_send(ClientFrame::Binary(vec![])),
        Err(client_plugin::ClientSendError::Full)
    );
    assert!(receiver.try_recv().is_ok());
    sender
        .try_send(ClientFrame::Close {
            code: 1000,
            reason: String::new(),
        })
        .unwrap();
    drop(receiver);
    assert!(sender.is_closed());
    assert_eq!(
        sender.try_send(ClientFrame::Text(String::new())),
        Err(client_plugin::ClientSendError::Closed)
    );
    assert_eq!(
        harness.register(id, "closed", "webui", None),
        Err(ClientError::ConnectionMissing)
    );
}

#[test]
fn duplicate_names_are_rejected_and_disconnect_is_idempotent() {
    let mut harness = Harness::new();
    let (first_rx, first) = harness.connect();
    let (second_rx, second) = harness.connect();
    let registered = harness
        .register(first, "first", "webui", Some("shared"))
        .unwrap();
    assert_eq!(
        harness.register(second, "second", "cli", Some("shared")),
        Err(ClientError::DuplicateName)
    );
    harness.disconnect(first_rx, first);
    harness.app.world().send_event(ClientDisconnected {
        connection_id: first,
    });
    harness.app.tick();
    harness.app.tick();
    assert!(!harness.app.world().is_alive(registered.client));
    assert!(harness
        .register(second, "retry", "webui", Some("shared"))
        .is_ok());
    harness.disconnect(second_rx, second);
}

#[test]
fn webui_sender_snapshots_deduplicate_connection_ids_and_skip_closed_capabilities() {
    let mut harness = Harness::new();
    let (receiver, connection) = harness.connect();
    let registered = harness
        .register(connection, "register", "webui", None)
        .unwrap();
    let client = harness
        .app
        .world()
        .get_component::<Client>(registered.client)
        .unwrap()
        .clone();
    let duplicate = harness.app.world_mut().spawn();
    harness.app.world_mut().insert_component(duplicate, client);
    let snapshot = harness.app.world().client_webui_senders();
    assert_eq!(snapshot.len(), 1);
    drop(receiver);
    assert!(snapshot[0].is_closed());
    assert!(harness.app.world().client_webui_senders().is_empty());
}
