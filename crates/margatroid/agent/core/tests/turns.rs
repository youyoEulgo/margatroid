use agent_core::{Agent, AgentErrorKind, AgentState, WorldAgentExt};
use core_plugin::World;

fn agent(world: &mut World) -> core_plugin::Entity {
    let workspace = world.spawn();
    let entity = world.spawn();
    world.insert_component(
        entity,
        agent_core::Agent::new(
            "coder".into(),
            workspace,
            AgentState::Idle,
            None,
            crate::test_agent_context(),
        ),
    );
    entity
}

#[test]
fn turn_admission_changes_state_synchronously_and_never_overwrites() {
    let mut world = World::new();
    let entity = agent(&mut world);
    let turn = world.agent_start_turn(entity).unwrap();
    assert_eq!(*world.agent_state(entity).unwrap(), AgentState::Working);
    assert_eq!(
        world.agent_start_turn(entity).unwrap_err().kind,
        AgentErrorKind::StateRejected
    );
    assert_eq!(
        world
            .get_component::<Agent>(entity)
            .unwrap()
            .turn()
            .as_ref(),
        Some(&turn)
    );
    assert!(world.agent_finish_turn(entity, "stale").is_err());
    world.agent_finish_turn(entity, &turn).unwrap();
    assert_eq!(*world.agent_state(entity).unwrap(), AgentState::Idle);
    assert!(world.agent_finish_turn(entity, &turn).is_err());
}

#[test]
fn abort_clears_turn_but_keeps_working_until_driver_finishes() {
    let mut world = World::new();
    let entity = agent(&mut world);
    let turn = world.agent_start_turn(entity).unwrap();
    world.agent_abort_turn(entity, &turn).unwrap();
    assert_eq!(*world.agent_state(entity).unwrap(), AgentState::Working);
    assert!(world
        .get_component::<Agent>(entity)
        .unwrap()
        .turn()
        .is_none());
    assert!(world.agent_abort_turn(entity, &turn).is_err());
    world.agent_finish_turn(entity, &turn).unwrap();
    assert_eq!(*world.agent_state(entity).unwrap(), AgentState::Idle);
}

#[test]
fn lifecycle_states_reject_turn_operations_and_old_entities_do_not_resolve() {
    let mut world = World::new();
    let entity = agent(&mut world);
    for state in [
        AgentState::Starting,
        AgentState::Restarting,
        AgentState::Stopping,
    ] {
        world.agent_transition(entity, state, None).unwrap();
        assert!(world.agent_start_turn(entity).is_err());
        assert!(world.agent_abort_turn(entity, "turn").is_err());
        assert!(world.agent_finish_turn(entity, "turn").is_err());
        assert_eq!(*world.agent_state(entity).unwrap(), state);
    }
    world.despawn(entity);
    let replacement = agent(&mut world);
    assert_ne!(entity, replacement);
    assert_eq!(
        world.agent_state(entity).unwrap_err().kind,
        AgentErrorKind::AgentMissing
    );
}

fn test_agent_context() -> agent_core::AgentUpdateContext {
    agent_core::AgentUpdateContext::new(|_, _, _| Vec::new(), |_, _| {})
}

#[test]
fn resource_probe_scope_is_retired_when_actual_agent_runtime_stops() {
    use std::sync::{Arc, Mutex};
    let mut world = World::new();
    let owner = world.spawn();
    let values = Arc::new(Mutex::new(Vec::new()));
    let recorded = values.clone();
    let context = agent_core::AgentUpdateContext::new(
        |_, _, _| Vec::new(),
        move |_, update| recorded.lock().unwrap().push(update),
    );
    let old = context.scoped(owner);
    old.update_public(
        owner,
        agent_core::AgentPublicUpdate::Sandbox(Some("old".into())),
    );
    context.notify(
        &mut world,
        owner,
        agent_core::AgentUpdateKind::RuntimeStopped,
    );
    old.update_public(
        owner,
        agent_core::AgentPublicUpdate::Sandbox(Some("stale".into())),
    );
    let current = context.scoped(owner);
    current.update_public(
        owner,
        agent_core::AgentPublicUpdate::Sandbox(Some("current".into())),
    );
    assert_eq!(values.lock().unwrap().len(), 2);
}
