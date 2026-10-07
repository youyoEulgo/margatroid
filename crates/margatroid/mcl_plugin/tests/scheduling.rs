use std::sync::Arc;

use agent_plugin::{
    Agent, AgentCreationState, AgentInferenceState, AgentInfo, AgentLifecycleState, AgentLuaState,
    AgentMemoryHandle, AgentMemoryStore, AgentMemoryStoreError, AgentModelInfo, AgentPlugin,
};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity, Plugin, World};
use lua_runtime_plugin::{
    LuaRuntimeHandle, LuaRuntimePlugin, LuaValue, LuaVmId, LuaVmMessageArrived, LuaVmOwner,
};
use margatroid_types::{AgentError, Message, ResourceId, TokenUsage, ToolDefinition};
use mcl_plugin::{
    MclCommandId, MclCommandReply, MclCommandRequest, MclDomainRequest, MclEffectCommand,
    MclOperation, MclPlugin, ENTRY_BLOCK_ID, ENTRY_INNER_ID,
};
use resource_id_plugin::ResourceIdPlugin;
use tool_plugin::ToolPlugin;

struct EmptyMemory;

impl AgentMemoryStore for EmptyMemory {
    fn set_state(&self, _key: &str, _value: &str) -> Result<(), AgentMemoryStoreError> {
        Ok(())
    }
    fn state_value(&self, _key: &str) -> Result<Option<String>, AgentMemoryStoreError> {
        Ok(None)
    }
    fn append_record(
        &self,
        _kind: &str,
        _content: &str,
        _payload: &str,
        _source: &str,
    ) -> Result<(), AgentMemoryStoreError> {
        Ok(())
    }
    fn append_history(
        &self,
        _turn_id: &str,
        _source: &str,
        _message: &Message,
        _tool_schema: &[ToolDefinition],
        _usage: Option<&TokenUsage>,
    ) -> Result<(), AgentMemoryStoreError> {
        Ok(())
    }
    fn history_messages(&self) -> Result<Vec<agent_plugin::HistoryMessage>, AgentMemoryStoreError> {
        Ok(Vec::new())
    }
}

fn build_app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourceIdPlugin)
        .add_plugin(LuaRuntimePlugin::default())
        .add_plugin(ToolPlugin::default())
        .add_plugin(MclPlugin::new())
        .add_plugin(AgentPlugin::default());
    app.tick();
    app
}

fn spawn_agent(world: &mut World, lifecycle: AgentLifecycleState, vm: Option<LuaVmId>) -> Entity {
    let workspace = world.spawn();
    let image = world.spawn();
    let entity = world.spawn();
    let id: ResourceId = "agent:test/coder:latest".parse().unwrap();
    let (sender, _receiver) = tokio::sync::oneshot::channel::<Result<Entity, AgentError>>();
    let model = AgentModelInfo {
        provider: "test".to_owned(),
        model: "test".to_owned(),
        context_window_tokens: 1024,
    };
    world.insert_component(entity, id);
    world.insert_component(
        entity,
        Agent {
            info: AgentInfo {
                image_entity: image,
                workspace_id: workspace,
                model: model.clone(),
                project_root: Default::default(),
                image_root: Default::default(),
                home_root: Default::default(),
                image_dependencies: Default::default(),
                image_sources: Default::default(),
            },
            creation: AgentCreationState {
                request_id: "create-1".to_owned(),
                reply: agent_plugin::AgentCreateReply::new(sender),
                initialization: Default::default(),
            },
            mcl: Default::default(),
            resources: Default::default(),
            memory: AgentMemoryHandle::new(Arc::new(EmptyMemory)),
            inference: AgentInferenceState {
                model,
                pending: Default::default(),
            },
            tools: Default::default(),
            lua: AgentLuaState {
                request_id: None,
                vm_id: vm,
            },
            lifecycle,
            turn: Default::default(),
            token_usage: Default::default(),
            last_error: None,
        },
    );
    entity
}

fn send_effect(app: &mut App, effect: MclEffectCommand) -> Result<(), mcl_plugin::MclError> {
    let (result, _events) = send_effect_collecting(app, effect);
    result
}

fn send_effect_collecting(
    app: &mut App,
    effect: MclEffectCommand,
) -> (
    Result<(), mcl_plugin::MclError>,
    Vec<agent_plugin::AgentInitializationCompleted>,
) {
    let (sender, mut receiver) = tokio::sync::oneshot::channel();
    app.world_mut().send_event(MclDomainRequest {
        id: MclCommandId::new("probe-1").unwrap(),
        source: "agent:test/coder:latest".to_owned(),
        agent_id: "agent:test/coder:latest".parse().unwrap(),
        operation: MclOperation::Emit { effect },
        reply: MclCommandReply::new(sender),
    });
    let mut answered = None;
    let mut events = Vec::new();
    for _ in 0..4 {
        app.tick();
        if answered.is_none() {
            if let Ok(result) = receiver.try_recv() {
                answered = Some(result.map(|_| ()));
            }
        }
        events.extend(
            app.world()
                .event_reader::<agent_plugin::AgentInitializationCompleted>()
                .into_iter()
                .cloned(),
        );
    }
    (answered.expect("the effect was never answered"), events)
}

fn turn_of(world: &World, agent: Entity) -> Option<String> {
    world
        .get_component::<Agent>(agent)
        .and_then(|agent| agent.turn.turn_id.clone())
}

#[test]
fn a_waiting_agent_is_released_by_wake() {
    let mut app = build_app();
    let handle = app
        .world()
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .expect("the Lua runtime is installed");
    let mut waiting = handle
        .suspend("agent:test/coder:latest".into())
        .expect("the agent may wait");
    assert!(waiting.try_recv().is_err(), "the agent is suspended");
    handle
        .wake("agent:test/coder:latest")
        .expect("the suspended agent is released");
    assert!(waiting.try_recv().is_ok(), "the agent resumes after wake");
}

#[test]
fn waking_an_agent_that_is_not_waiting_is_rejected() {
    let app = build_app();
    let handle = app
        .world()
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .expect("the Lua runtime is installed");
    assert!(handle.wake("agent:test/coder:latest").is_err());
}

#[test]
fn one_agent_may_only_wait_once_at_a_time() {
    let app = build_app();
    let handle = app
        .world()
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .expect("the Lua runtime is installed");
    let _first = handle
        .suspend("agent:test/coder:latest".into())
        .expect("the first wait is accepted");
    assert!(
        handle.suspend("agent:test/coder:latest".into()).is_err(),
        "a second wait on the same agent is rejected"
    );
}

#[test]
fn the_entry_block_and_inner_have_stable_names() {
    assert_eq!(ENTRY_BLOCK_ID, "entry");
    assert_eq!(ENTRY_INNER_ID, "pending");
}

#[test]
fn the_mcl_environment_provides_wait() {
    let app = build_app();
    let handle = app
        .world()
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .expect("the Lua runtime is installed");
    let environment = handle
        .environments()
        .read()
        .expect("the environment registry is readable")
        .collect(
            &["mcl".to_owned()],
            &lua_runtime_plugin::LuaEnvironmentContext {
                request_id: "test".into(),
                owner: LuaVmOwner {
                    owner_id: "agent:test/coder:latest".into(),
                },
                values: Default::default(),
            },
        )
        .expect("the mcl environment is provided");
    let names = environment
        .globals
        .iter()
        .map(|binding| binding.name.clone())
        .collect::<Vec<_>>();
    assert!(names.contains(&"mcl".to_owned()), "names: {names:?}");
    assert!(names.contains(&"mcl_wait".to_owned()), "names: {names:?}");
}

#[test]
fn a_tick_does_not_release_a_waiting_agent() {
    let mut app = build_app();
    let handle = app
        .world()
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .expect("the Lua runtime is installed");
    let mut waiting = handle
        .suspend("agent:test/coder:latest".into())
        .expect("the agent may wait");
    app.tick();
    assert!(
        waiting.try_recv().is_err(),
        "only an explicit wake resumes the agent"
    );
}

#[test]
fn wake_releases_an_agent_through_the_effect() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(1)),
    );
    let handle = app
        .world()
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .expect("the Lua runtime is installed");
    let mut waiting = handle
        .suspend("agent:test/coder:latest".into())
        .expect("the agent may wait");
    send_effect(&mut app, MclEffectCommand::Wake).expect("wake reaches the waiting agent");
    assert!(waiting.try_recv().is_ok(), "the waiting call returns");
}

#[test]
fn wake_rejects_an_agent_that_is_not_waiting() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(2)),
    );
    let error = send_effect(&mut app, MclEffectCommand::Wake).unwrap_err();
    assert!(matches!(error, mcl_plugin::MclError::EffectInvalid));
}

#[test]
fn wake_reports_a_missing_agent() {
    let mut app = build_app();
    let error = send_effect(&mut app, MclEffectCommand::Wake).unwrap_err();
    assert!(matches!(error, mcl_plugin::MclError::AgentMissing));
}

#[test]
fn ready_completes_a_creating_agent() {
    let mut app = build_app();
    let agent = spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Creating,
        Some(LuaVmId(3)),
    );
    let (result, events) = send_effect_collecting(&mut app, MclEffectCommand::Ready);
    result.expect("ready is accepted");
    assert!(
        events.iter().any(|event| event.agent == agent),
        "ready announces the initialization"
    );
}

#[test]
fn ready_is_quiet_for_an_agent_that_is_already_running() {
    let mut app = build_app();
    let agent = spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(4)),
    );
    let (result, events) = send_effect_collecting(&mut app, MclEffectCommand::Ready);
    result.expect("ready is accepted");
    assert!(
        !events.iter().any(|event| event.agent == agent),
        "a running agent is not announced again"
    );
}

#[test]
fn ready_reports_a_missing_agent() {
    let mut app = build_app();
    let error = send_effect(&mut app, MclEffectCommand::Ready).unwrap_err();
    assert!(matches!(error, mcl_plugin::MclError::AgentMissing));
}

#[test]
fn start_turn_opens_a_turn_and_end_turn_closes_it() {
    let mut app = build_app();
    let agent = spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(5)),
    );
    assert!(turn_of(app.world(), agent).is_none());
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    assert!(turn_of(app.world(), agent).is_some(), "the turn is open");
    send_effect(&mut app, MclEffectCommand::EndTurn).expect("the turn ends");
    assert!(turn_of(app.world(), agent).is_none(), "the turn is closed");
}

#[test]
fn a_second_turn_cannot_start_while_one_is_open() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(6)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let error = send_effect(&mut app, MclEffectCommand::StartTurn).unwrap_err();
    assert!(matches!(error, mcl_plugin::MclError::TurnMismatch));
}

#[test]
fn ending_a_turn_without_one_open_is_rejected() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(7)),
    );
    let error = send_effect(&mut app, MclEffectCommand::EndTurn).unwrap_err();
    assert!(matches!(error, mcl_plugin::MclError::TurnMissing));
}

#[test]
fn a_turn_can_be_reopened_after_it_closes() {
    let mut app = build_app();
    let agent = spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(8)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("the first turn starts");
    let first = turn_of(app.world(), agent).expect("the first turn is open");
    send_effect(&mut app, MclEffectCommand::EndTurn).expect("the first turn ends");
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("the second turn starts");
    let second = turn_of(app.world(), agent).expect("the second turn is open");
    assert_ne!(first, second, "each turn gets its own identity");
}

#[test]
fn a_delivery_without_a_matching_agent_is_ignored() {
    let mut app = build_app();
    app.world_mut().send_event(LuaVmMessageArrived {
        vm_id: LuaVmId(99),
        owner: LuaVmOwner {
            owner_id: "agent:missing/ghost:latest".to_owned(),
        },
        value: LuaValue::Nil,
    });
    for _ in 0..3 {
        app.tick();
    }
    let deliveries = app
        .world()
        .event_reader::<MclCommandRequest>()
        .into_iter()
        .count();
    assert_eq!(deliveries, 0, "nothing is delivered for an unknown agent");
}

#[test]
fn a_delivery_targets_the_entry_block() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(10)),
    );
    let envelope = margatroid_types::AgentLuaMessageEnvelope {
        turn_id: "turn-1".to_owned(),
        message: margatroid_types::MclMessage::new(
            Message::User {
                content: "hello".to_owned(),
            },
            None,
        ),
    };
    let value = json_to_lua(serde_json::to_value(&envelope).unwrap());
    app.world_mut().send_event(LuaVmMessageArrived {
        vm_id: LuaVmId(10),
        owner: LuaVmOwner {
            owner_id: "agent:test/coder:latest".to_owned(),
        },
        value,
    });
    let mut commands = Vec::new();
    for _ in 0..4 {
        app.tick();
        commands.extend(
            app.world()
                .event_reader::<MclCommandRequest>()
                .into_iter()
                .map(|request| request.command.clone()),
        );
    }
    assert!(
        commands
            .iter()
            .any(|command| command.contains("entry.pending[-1]")),
        "the delivery reaches the entry block: {commands:?}"
    );
}

#[test]
fn a_delivery_carries_the_payload_of_the_message() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(11)),
    );
    let envelope = margatroid_types::AgentLuaMessageEnvelope {
        turn_id: "turn-1".to_owned(),
        message: margatroid_types::MclMessage::new(
            Message::User {
                content: "carried".to_owned(),
            },
            None,
        ),
    };
    let value = json_to_lua(serde_json::to_value(&envelope).unwrap());
    app.world_mut().send_event(LuaVmMessageArrived {
        vm_id: LuaVmId(11),
        owner: LuaVmOwner {
            owner_id: "agent:test/coder:latest".to_owned(),
        },
        value,
    });
    let mut payloads = Vec::new();
    for _ in 0..4 {
        app.tick();
        payloads.extend(
            app.world()
                .event_reader::<MclCommandRequest>()
                .into_iter()
                .filter_map(|request| request.binding.clone()),
        );
    }
    let carried = payloads
        .iter()
        .any(|payload| payload.get("content").and_then(|v| v.as_str()) == Some("carried"));
    assert!(
        carried,
        "the message content reaches the entry block: {payloads:?}"
    );
}

#[test]
fn a_malformed_delivery_is_ignored() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(12)),
    );
    app.world_mut().send_event(LuaVmMessageArrived {
        vm_id: LuaVmId(12),
        owner: LuaVmOwner {
            owner_id: "agent:test/coder:latest".to_owned(),
        },
        value: LuaValue::String("not an envelope".to_owned()),
    });
    for _ in 0..3 {
        app.tick();
    }
    let deliveries = app
        .world()
        .event_reader::<MclCommandRequest>()
        .into_iter()
        .count();
    assert_eq!(deliveries, 0, "a malformed message is not delivered");
}

#[test]
fn the_runtime_is_reachable_from_the_world_after_a_tick() {
    let mut app = build_app();
    for _ in 0..3 {
        app.tick();
    }
    assert!(app.world().get_resource::<LuaRuntimeHandle>().is_some());
}

fn json_to_lua(value: serde_json::Value) -> LuaValue {
    match value {
        serde_json::Value::Null => LuaValue::Nil,
        serde_json::Value::Bool(value) => LuaValue::Boolean(value),
        serde_json::Value::Number(value) => match value.as_i64() {
            Some(value) => LuaValue::Integer(value),
            None => LuaValue::Number(value.as_f64().unwrap_or_default()),
        },
        serde_json::Value::String(value) => LuaValue::String(value),
        serde_json::Value::Array(values) => {
            LuaValue::Array(values.into_iter().map(json_to_lua).collect())
        }
        serde_json::Value::Object(values) => LuaValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, json_to_lua(value)))
                .collect(),
        ),
    }
}
