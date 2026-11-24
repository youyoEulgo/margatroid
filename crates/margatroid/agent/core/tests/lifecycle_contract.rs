use agent_core::*;
use core_plugin::World;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn closing_wakes_ready_and_waiters_before_entity_is_removed() {
    let mut world = World::default();
    let workspace = world.spawn();
    let parent = workspace;
    let agent = Agent::new(
        workspace,
        parent,
        "task-1".into(),
        std::path::PathBuf::from("/project"),
    );
    let ready = agent.ready_receiver();
    let wake = agent.wake_slot();
    let entity = world.spawn();
    world.insert_component(entity, agent);
    let handle = AgentHandle::new(entity, ready, || Box::pin(async { Ok(()) }));
    world.agent_begin_close(entity).unwrap();
    assert!(handle.wait_ready().await.is_err());
    assert!(AgentWakeSlot::wait(wake).await.is_err());
    assert_eq!(
        world.get_component::<Agent>(entity).unwrap().state(),
        AgentState::Closing
    );
    assert!(world.agent_ready(entity).is_err());
}

#[tokio::test]
async fn wake_before_wait_is_consumed_once_and_cancelled_wait_releases_slot() {
    let slot = Arc::new(Mutex::new(AgentWakeSlot::new()));
    slot.lock().unwrap().wake().unwrap();
    AgentWakeSlot::wait(slot.clone()).await.unwrap();
    let mut waiter = Box::pin(AgentWakeSlot::wait(slot.clone()));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1), &mut waiter)
            .await
            .is_err()
    );
    assert!(slot.lock().unwrap().is_waiting());
    drop(waiter);
    assert!(!slot.lock().unwrap().is_waiting());
    slot.lock().unwrap().wake().unwrap();
    AgentWakeSlot::wait(slot).await.unwrap();
}

#[test]
fn task_identity_and_turns_survive_ready_idempotence() {
    let mut world = World::default();
    let parent = world.spawn();
    let e = world.spawn();
    world.insert_component(
        e,
        Agent::new(parent, parent, "actual-task".into(), "/project".into()),
    );
    assert!(world.agent_start_turn(e).is_err());
    world.agent_ready(e).unwrap();
    let turn = world.agent_start_turn(e).unwrap();
    world.agent_ready(e).unwrap();
    assert_eq!(world.agent_start_turn(e).unwrap(), turn);
    assert!(world.agent_abort_turn(e, "wrong").is_err());
    world.agent_finish_turn(e, &turn).unwrap();
    assert!(world.get_component::<Agent>(e).unwrap().turn().is_none());
    assert_eq!(
        world.get_component::<Agent>(e).unwrap().task_owner(),
        (parent, "actual-task")
    );
}
