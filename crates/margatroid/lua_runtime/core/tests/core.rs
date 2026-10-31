use core_plugin::World;
use lua_runtime_core::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[tokio::test]
async fn early_wakes_coalesce_and_live_waiters_are_notified() {
    let slot = DriverWakeSlot::shared();
    slot.lock().unwrap().wake().unwrap();
    slot.lock().unwrap().wake().unwrap();
    DriverWakeSlot::wait(slot.clone()).await.unwrap();
    let mut waiter = Box::pin(DriverWakeSlot::wait(slot.clone()));
    assert!(poll_once(waiter.as_mut()).is_pending());
    assert!(slot.lock().unwrap().is_waiting());
    slot.lock().unwrap().wake().unwrap();
    waiter.await.unwrap();
    assert!(!slot.lock().unwrap().is_waiting());
}

#[tokio::test]
async fn concurrent_wait_is_rejected_and_dropped_wait_clears_registration() {
    let slot = DriverWakeSlot::shared();
    let mut first = Box::pin(DriverWakeSlot::wait(slot.clone()));
    assert!(poll_once(first.as_mut()).is_pending());
    assert_eq!(
        DriverWakeSlot::wait(slot.clone()).await.unwrap_err().kind,
        LuaRuntimeErrorKind::AlreadyWaiting
    );
    drop(first);
    assert!(!slot.lock().unwrap().is_waiting());
    let mut replacement = Box::pin(DriverWakeSlot::wait(slot.clone()));
    assert!(poll_once(replacement.as_mut()).is_pending());
    slot.lock().unwrap().wake().unwrap();
    replacement.await.unwrap();
}

#[tokio::test]
async fn close_ends_pending_wait_and_rejects_new_waits_and_wakes() {
    let slot = DriverWakeSlot::shared();
    let mut waiter = Box::pin(DriverWakeSlot::wait(slot.clone()));
    assert!(poll_once(waiter.as_mut()).is_pending());
    slot.lock().unwrap().close();
    assert_eq!(waiter.await.unwrap_err().kind, LuaRuntimeErrorKind::Closed);
    assert!(!slot.lock().unwrap().is_waiting());
    assert!(slot.lock().unwrap().is_closed());
    assert_eq!(
        DriverWakeSlot::wait(slot.clone()).await.unwrap_err().kind,
        LuaRuntimeErrorKind::Closed
    );
    assert_eq!(
        slot.lock().unwrap().wake().unwrap_err().kind,
        LuaRuntimeErrorKind::Closed
    );
}

#[tokio::test]
async fn wake_after_delivery_is_latched_for_next_wait() {
    let slot = DriverWakeSlot::shared();
    let mut first = Box::pin(DriverWakeSlot::wait(slot.clone()));
    assert!(poll_once(first.as_mut()).is_pending());
    slot.lock().unwrap().wake().unwrap();
    slot.lock().unwrap().wake().unwrap();
    first.await.unwrap();
    DriverWakeSlot::wait(slot.clone()).await.unwrap();
    let mut third = Box::pin(DriverWakeSlot::wait(slot));
    assert!(poll_once(third.as_mut()).is_pending());
}

fn install(
    world: &mut World,
    owner: core_plugin::Entity,
    start: &str,
) -> (
    DriverLuaVmInstance,
    oneshot::Receiver<Result<(), LuaRuntimeError>>,
) {
    let instance = DriverLuaVmInstance {
        owner,
        start_id: start.into(),
    };
    let (ready, receiver) = oneshot::channel();
    let (_ended, ended) = oneshot::channel();
    assert!(world.insert_component(
        owner,
        DriverLuaVm {
            instance: instance.clone(),
            wake: DriverWakeSlot::shared(),
            ready: Arc::new(Mutex::new(Some(ready))),
            ended: Some(ended),
            cancel: CancellationToken::new(),
        }
    ));
    (instance, receiver)
}

#[tokio::test]
async fn world_notifications_check_instance_and_ready_is_consumed_once() {
    let mut world = World::new();
    let owner = world.spawn();
    let (instance, ready) = install(&mut world, owner, "new");
    let stale = DriverLuaVmInstance {
        owner,
        start_id: "old".into(),
    };
    assert_eq!(
        world.driver_ready(&stale).unwrap_err().kind,
        LuaRuntimeErrorKind::InstanceMismatch
    );
    assert_eq!(
        world.driver_wake(&stale).unwrap_err().kind,
        LuaRuntimeErrorKind::InstanceMismatch
    );
    world.driver_ready(&instance).unwrap();
    ready.await.unwrap().unwrap();
    assert_eq!(
        world.driver_ready(&instance).unwrap_err().kind,
        LuaRuntimeErrorKind::AlreadyReady
    );
    world.driver_wake(&instance).unwrap();
    let slot = world
        .get_component::<DriverLuaVm>(owner)
        .unwrap()
        .wake
        .clone();
    DriverWakeSlot::wait(slot).await.unwrap();
}

#[test]
fn full_entity_generation_prevents_stale_notifications() {
    let mut world = World::new();
    let old_owner = world.spawn();
    let (old, _) = install(&mut world, old_owner, "same-id");
    assert!(world.despawn(old_owner));
    let owner = world.spawn();
    assert_eq!(owner.index(), old_owner.index());
    assert_ne!(owner.generation(), old_owner.generation());
    let (current, _ready) = install(&mut world, owner, "same-id");
    assert_eq!(
        world.driver_ready(&old).unwrap_err().kind,
        LuaRuntimeErrorKind::InstanceNotFound
    );
    assert_eq!(
        world.driver_wake(&old).unwrap_err().kind,
        LuaRuntimeErrorKind::InstanceNotFound
    );
    world.driver_ready(&current).unwrap();
}

#[test]
fn json_bridge_preserves_array_object_identity_and_rejects_unrepresentable_numbers() {
    let value = json!({"array": [], "object": {}, "nested": [[]], "nil": null, "integer": -42, "number": 1.5, "string": "你好", "bool": true});
    let lua = LuaValue::from_json(value.clone()).unwrap();
    assert_eq!(lua.to_json().unwrap(), value);
    let LuaValue::Object(object) = lua else {
        panic!("object");
    };
    assert_eq!(object["array"], LuaValue::Array(vec![]));
    assert_eq!(object["object"], LuaValue::Object(BTreeMap::new()));
    assert_eq!(
        object["nested"],
        LuaValue::Array(vec![LuaValue::Array(vec![])])
    );
    assert_eq!(
        LuaValue::from_json(json!(u64::MAX)).unwrap_err().kind,
        LuaRuntimeErrorKind::InvalidRequest
    );
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            LuaValue::Number(number).to_json().unwrap_err().kind,
            LuaRuntimeErrorKind::InvalidRequest
        );
    }
}

#[test]
fn execution_budgets_reject_zero_values() {
    let defaults = LuaRuntimeConfig::default();
    defaults.validate().unwrap();
    for invalid in [
        LuaRuntimeConfig {
            max_source_bytes: 0,
            ..defaults.clone()
        },
        LuaRuntimeConfig {
            max_memory_bytes: 0,
            ..defaults.clone()
        },
        LuaRuntimeConfig {
            instruction_check_interval: 0,
            ..defaults.clone()
        },
        LuaRuntimeConfig {
            startup_timeout: Some(Duration::ZERO),
            ..defaults
        },
    ] {
        assert_eq!(
            invalid.validate().unwrap_err().kind,
            LuaRuntimeErrorKind::InvalidRequest
        );
    }
}

struct Echo;
impl LuaHostFunction for Echo {
    fn call(&self, arguments: Vec<LuaValue>, cancel: CancellationToken) -> HostFuture {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(LuaRuntimeError::new(
                    LuaRuntimeErrorKind::Cancelled,
                    "cancelled",
                ));
            }
            Ok(LuaValue::Array(arguments))
        })
    }
}

#[tokio::test]
async fn environment_hosts_use_owned_send_futures_and_generic_values() {
    let host: Arc<dyn LuaHostFunction> = Arc::new(Echo);
    let environment = LuaEnvironment {
        globals: BTreeMap::from([("value".into(), LuaBinding::Value(LuaValue::Integer(7)))]),
        modules: BTreeMap::from([("echo".into(), LuaBinding::Function(host.clone()))]),
    };
    assert!(format!("{environment:?}").contains("Function"));
    assert_eq!(
        host.call(vec![LuaValue::Array(vec![])], CancellationToken::new())
            .await
            .unwrap(),
        LuaValue::Array(vec![LuaValue::Array(vec![])])
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        host.call(vec![], cancel).await.unwrap_err().kind,
        LuaRuntimeErrorKind::Cancelled
    );
}

#[tokio::test]
async fn events_share_single_oneshot_reply_and_ended_has_one_receiver() {
    fn event<T: core_plugin::Event>() {}
    event::<StartDriverLuaVm>();
    event::<StopDriverLuaVm>();
    event::<DriverLuaVmExited>();
    let mut world = World::new();
    let owner = world.spawn();
    let workspace = world.spawn();
    let (sender, receiver) = oneshot::channel();
    let request = StartDriverLuaVm {
        owner,
        workspace,
        program: LuaProgram {
            source: "return".into(),
            origin: "driver.lua".into(),
            libraries: LuaStandardLibraries::Minimal,
        },
        environment: LuaEnvironment::default(),
        reply: Arc::new(Mutex::new(Some(sender))),
    };
    let cloned = request.clone();
    let instance = DriverLuaVmInstance {
        owner,
        start_id: "start".into(),
    };
    cloned
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(instance.clone()))
        .unwrap();
    assert!(request.reply.lock().unwrap().is_none());
    assert_eq!(receiver.await.unwrap().unwrap(), instance);
    install(&mut world, owner, "second");
    let vm = world.get_component_mut::<DriverLuaVm>(owner).unwrap();
    assert!(vm.ended.take().is_some());
    assert!(vm.ended.take().is_none());
}
