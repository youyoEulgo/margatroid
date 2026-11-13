use client_core::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

fn register(clients: &Clients, kind: &str) -> (ConnectionId, ClientReceiver) {
    let (id, receiver, _) = clients.allocate().unwrap();
    let mut app = core_plugin::App::new();
    let entity = app.world_mut().spawn();
    clients
        .commit_registration(
            id,
            Arc::new(ClientIdentity {
                client: entity,
                address: format!("client:{kind}/test:{}", id.get()),
                client_type: kind.into(),
                name: format!("test-{}", id.get()),
            }),
        )
        .unwrap();
    clients.initialize(id);
    (id, receiver)
}
fn context(clients: Clients, state: Arc<AtomicU64>) -> SyncContext<u64> {
    SyncContext::new(
        clients,
        SyncKey {
            topic: "test.sync".into(),
            object: "one".into(),
        },
        Arc::from([ClientSelector::Type("webui".into())]),
        move || {
            let value = state.load(Ordering::Acquire);
            Ok((value, value))
        },
        |value| Ok(Arc::new(ClientFrame::Text(value.to_string()))),
    )
}
#[tokio::test]
async fn changing_state_coalesces_in_one_existing_queue_and_late_clients_get_latest() {
    let clients = Clients::default();
    assert!(clients.same_instance(&clients.clone()));
    let (id, mut first) = register(&clients, "webui");
    let (_, mut excluded) = register(&clients, "cli");
    let state = Arc::new(AtomicU64::new(0));
    let sync = context(clients.clone(), state.clone());
    for revision in 1..=10000 {
        state.store(revision, Ordering::Release);
        sync.publish(revision);
    }
    assert!(!clients.get(id).unwrap().is_closed());
    assert_eq!(first.recv().await, Some(ClientFrame::Text("10000".into())));
    assert!(first.try_recv().is_err());
    assert!(excluded.try_recv().is_err());
    let (_, mut late) = register(&clients, "webui");
    assert_eq!(late.recv().await, Some(ClientFrame::Text("10000".into())));
    first.close().await;
    late.close().await;
    excluded.close().await;
    sync.close();
}
#[test]
fn unregistered_and_removed_connections_are_never_selected() {
    let clients = Clients::default();
    let (id, receiver, _) = clients.allocate().unwrap();
    assert!(clients.select(&[ClientSelector::Broadcast]).is_empty());
    assert!(clients.remove(id));
    assert!(!clients.remove(id));
    let mut app = core_plugin::App::new();
    let entity = app.world_mut().spawn();
    assert_eq!(
        clients.commit_registration(
            id,
            Arc::new(ClientIdentity {
                client: entity,
                address: "client:webui/stale".into(),
                client_type: "webui".into(),
                name: "stale".into()
            })
        ),
        Err(ClientError::ConnectionMissing)
    );
    drop(receiver);
}
#[tokio::test]
async fn replacing_a_source_does_not_deliver_queued_old_data() {
    let clients = Clients::default();
    let (_, mut receiver) = register(&clients, "webui");
    let old = context(clients.clone(), Arc::new(AtomicU64::new(1)));
    old.publish(1);
    let new = context(clients.clone(), Arc::new(AtomicU64::new(2)));
    new.publish(2);
    old.publish(99);
    assert_eq!(receiver.recv().await, Some(ClientFrame::Text("2".into())));
    assert!(receiver.try_recv().is_err());
    receiver.close().await;
    new.close();
}
#[tokio::test]
async fn closing_receiver_waits_for_actual_state_preparation_worker() {
    let clients = Clients::default();
    let (_, mut receiver) = register(&clients, "webui");
    let started = Arc::new(AtomicU64::new(0));
    let signal = started.clone();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let worker = gate.clone();
    let sync = SyncContext::new(
        clients,
        SyncKey {
            topic: "blocked.sync".into(),
            object: "one".into(),
        },
        Arc::from([ClientSelector::Broadcast]),
        move || {
            signal.store(1, Ordering::Release);
            let (lock, notify) = &*worker;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = notify.wait(released).unwrap();
            }
            signal.store(2, Ordering::Release);
            Ok((1, 1u64))
        },
        |_| Ok(Arc::new(ClientFrame::Text("done".into()))),
    );
    sync.publish(1);
    {
        let mut read = Box::pin(receiver.recv());
        tokio::select! {value=&mut read=>panic!("unexpected completion: {value:?}"),_=async{while started.load(Ordering::Acquire)==0{tokio::task::yield_now().await;}}=>()}
    }
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        let (lock, notify) = &*gate;
        *lock.lock().unwrap() = true;
        notify.notify_all();
    });
    receiver.close().await;
    assert_eq!(started.load(Ordering::Acquire), 2);
    release.join().unwrap();
    sync.close();
}
#[tokio::test]
async fn ordered_frames_are_not_replaced_by_state_coalescing() {
    let clients = Clients::default();
    let (id, mut receiver) = register(&clients, "webui");
    let state = Arc::new(AtomicU64::new(1));
    let sync = context(clients.clone(), state.clone());
    let sender = clients.get(id).unwrap();
    assert_eq!(receiver.recv().await, Some(ClientFrame::Text("1".into())));
    sender.try_send(ClientFrame::Text("first".into())).unwrap();
    sync.publish(1);
    state.store(2, Ordering::Release);
    sync.publish(2);
    sender.try_send(ClientFrame::Text("last".into())).unwrap();
    assert_eq!(
        receiver.recv().await,
        Some(ClientFrame::Text("first".into()))
    );
    assert_eq!(receiver.recv().await, Some(ClientFrame::Text("2".into())));
    assert_eq!(
        receiver.recv().await,
        Some(ClientFrame::Text("last".into()))
    );
    receiver.close().await;
    sync.close();
}
