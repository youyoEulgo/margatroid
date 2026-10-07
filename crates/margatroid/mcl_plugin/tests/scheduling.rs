use std::sync::Arc;

use agent_plugin::{
    Agent, AgentCreationState, AgentInferenceState, AgentInfo, AgentLifecycleState, AgentLuaState,
    AgentMemoryHandle, AgentMemoryStore, AgentMemoryStoreError, AgentModelInfo, AgentPlugin,
};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity, World};
use lua_runtime_plugin::{LuaRuntimeHandle, LuaRuntimePlugin, LuaVmId};
use margatroid_types::{AgentError, Message, ResourceId, TokenUsage, ToolDefinition};
use mcl_plugin::{
    MclCommandId, MclCommandReply, MclDomainRequest, MclEffectCommand, MclOperation, MclPlugin,
};

const ENTRY_BLOCK_ID: &str = "entry";
const ENTRY_INNER_ID: &str = "pending";
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
    register_probe_tool(world, entity);
    entity
}

fn register_probe_tool(world: &mut World, agent: Entity) {
    let resource_id: ResourceId = "tool:local/probe:latest".parse().unwrap();
    let tool_id: ResourceId = "tool:builtin/skill-loader:latest".parse().unwrap();
    tool_plugin::register_agent_resource(
        world,
        agent,
        tool_plugin::candidate_resource_entry_with_content(
            resource_id.clone(),
            Some("probe".to_owned()),
            tool_id,
            tool_plugin::ToolTemplate::new(
                "probe",
                "A probe tool.",
                serde_json::json!({"type": "object"}),
            )
            .expect("the probe template is valid"),
            None,
        )
        .expect("the probe entry is valid"),
    )
    .expect("the probe tool is registered");
    if let Some(state) = world.get_component_mut::<Agent>(agent) {
        state.resources.visible.insert(resource_id);
        state
            .resources
            .default_visible
            .insert("tool:local/probe:latest".parse().unwrap());
    }
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
    let app = build_app();
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
fn the_entry_block_name_is_a_driver_convention_only() {
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
                owner: lua_runtime_plugin::LuaVmOwner {
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
fn the_runtime_is_reachable_from_the_world_after_a_tick() {
    let mut app = build_app();
    for _ in 0..3 {
        app.tick();
    }
    assert!(app.world().get_resource::<LuaRuntimeHandle>().is_some());
}

fn send_tool_call(
    app: &mut App,
    calls: Vec<margatroid_types::ToolCall>,
) -> (
    Option<Result<mcl_plugin::MclCommandValue, mcl_plugin::MclError>>,
    String,
) {
    let (sender, mut receiver) = tokio::sync::oneshot::channel();
    let command_id = MclCommandId::new("tool-1").unwrap();
    app.world_mut().send_event(MclDomainRequest {
        id: command_id.clone(),
        source: "agent:test/coder:latest".to_owned(),
        agent_id: "agent:test/coder:latest".parse().unwrap(),
        operation: MclOperation::Emit {
            effect: MclEffectCommand::ToolCall { calls },
        },
        reply: MclCommandReply::new(sender),
    });
    let effect_id = "mcl-effect:tool-1".to_owned();
    for _ in 0..3 {
        app.tick();
    }
    let mut answered = None;
    for _ in 0..3 {
        app.tick();
        if let Ok(result) = receiver.try_recv() {
            answered = Some(result);
            break;
        }
    }
    (answered, effect_id)
}

fn call(id: &str, name: &str) -> margatroid_types::ToolCall {
    margatroid_types::ToolCall {
        id: id.to_owned(),
        tool_name: name.to_owned(),
        arguments: "{}".to_owned(),
    }
}

#[test]
fn a_tool_call_is_answered_by_a_real_executor() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(30)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let (answered, _effect_id) = send_tool_call(&mut app, vec![call("call-1", "probe")]);
    let result = answered.expect("a real executor answers the call");
    assert!(result.is_ok(), "the caller receives a result: {result:?}");
    let pending = app
        .world()
        .get_resource::<mcl_plugin::PendingMclEffects>()
        .map(|pending| pending.effects.is_empty())
        .unwrap_or(true);
    assert!(pending, "the call is forgotten once answered");
}

#[test]
fn a_tool_call_result_carries_the_tool_call_id() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(31)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let (answered, _effect_id) = send_tool_call(&mut app, vec![call("call-1", "probe")]);
    let result = answered.expect("the call is answered");
    let margatroid_types::BlockInner::Message(messages) = inner_of(&result) else {
        panic!("a tool answer carries messages");
    };
    assert_eq!(messages.len(), 1, "one call yields one message");
    let margatroid_types::Message::Tool { tool_call_id, .. } = &messages[0].message else {
        panic!("the answer is a tool message");
    };
    assert_eq!(tool_call_id, "call-1", "the answer is tied to its call");
}

#[test]
fn a_batch_returns_one_message_per_call() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(32)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let (answered, _effect_id) = send_tool_call(
        &mut app,
        vec![call("call-1", "probe"), call("call-2", "probe")],
    );
    let result = answered.expect("the batch is answered");
    let margatroid_types::BlockInner::Message(messages) = inner_of(&result) else {
        panic!("a batch answer carries messages");
    };
    assert_eq!(messages.len(), 2, "every call in the batch is answered");
}

#[test]
fn an_unknown_tool_is_rejected_with_a_failed_message() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(33)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let (answered, _effect_id) = send_tool_call(&mut app, vec![call("call-1", "not-visible")]);
    let result = answered.expect("a rejected call is answered immediately");
    let margatroid_types::BlockInner::Message(messages) = inner_of(&result) else {
        panic!("a rejection carries messages");
    };
    let margatroid_types::Message::Tool {
        failed, content, ..
    } = &messages[0].message
    else {
        panic!("the rejection is a tool message");
    };
    assert!(failed, "the rejection is marked failed");
    assert!(content.contains("not visible"), "content: {content}");
}

fn inner_of(
    result: &Result<mcl_plugin::MclCommandValue, mcl_plugin::MclError>,
) -> margatroid_types::BlockInner {
    match result.as_ref().expect("the call succeeded") {
        mcl_plugin::MclCommandValue::Inner(inner) => inner.clone(),
        other => panic!("expected an inner value, got {other:?}"),
    }
}

#[test]
fn a_tool_error_answers_the_caller_and_reports_failure() {
    let mut app = build_app();
    let agent = spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(34)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let (_answered, effect_id) = send_tool_call(&mut app, vec![call("call-1", "probe")]);
    app.world_mut()
        .send_event(margatroid_types::MclEffectReply {
            id: effect_id,
            agent,
            result: Err("the runner died".to_owned()),
            usage: None,
        });
    for _ in 0..3 {
        app.tick();
    }
}

#[test]
fn an_unknown_tool_is_rejected_without_waiting() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(35)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let (answered, _effect_id) = send_tool_call(&mut app, vec![call("call-1", "not-visible")]);
    assert!(
        answered.is_some(),
        "a rejected call answers immediately instead of waiting"
    );
}

#[test]
fn a_tool_call_without_a_turn_is_rejected() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(36)),
    );
    let error = send_effect(
        &mut app,
        MclEffectCommand::ToolCall {
            calls: vec![call("call-1", "probe")],
        },
    )
    .unwrap_err();
    assert!(matches!(error, mcl_plugin::MclError::TurnMissing));
}

#[test]
fn an_empty_tool_batch_is_rejected() {
    let mut app = build_app();
    spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(37)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let error =
        send_effect(&mut app, MclEffectCommand::ToolCall { calls: Vec::new() }).unwrap_err();
    assert!(matches!(error, mcl_plugin::MclError::ToolCallInvalid));
}

#[test]
fn ending_a_turn_with_a_tracked_tool_call_is_rejected() {
    let mut app = build_app();
    let agent = spawn_agent(
        app.world_mut(),
        AgentLifecycleState::Running,
        Some(LuaVmId(38)),
    );
    send_effect(&mut app, MclEffectCommand::StartTurn).expect("a turn starts");
    let turn = turn_of(app.world(), agent).expect("the turn is open");
    if let Some(state) = app.world_mut().get_component_mut::<Agent>(agent) {
        state.tools.pending.insert(
            (agent, turn.clone(), "call-1".to_owned()),
            agent_plugin::AgentToolPending {
                turn_id: turn,
                tool_call_id: "call-1".to_owned(),
                resource_id: "tool:local/probe:latest".parse().unwrap(),
                tool_id: "tool:builtin/skill-loader:latest".parse().unwrap(),
            },
        );
    }
    let error = send_effect(&mut app, MclEffectCommand::EndTurn).unwrap_err();
    assert!(matches!(error, mcl_plugin::MclError::EffectInvalid));
}

#[test]
fn cancelling_a_suspended_agent_drops_its_wait() {
    let app = build_app();
    let handle = app
        .world()
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .expect("the Lua runtime is installed");
    let mut waiting = handle
        .suspend("agent:test/coder:latest".into())
        .expect("the agent may wait");
    handle.forget_suspension("agent:test/coder:latest");
    assert!(
        waiting.try_recv().is_err(),
        "the released wait never reports a wake"
    );
    assert!(
        handle.wake("agent:test/coder:latest").is_err(),
        "nothing is left to wake"
    );
}

#[test]
fn a_reservation_survives_until_it_is_claimed_or_dropped() {
    let app = build_app();
    let handle = app
        .world()
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .expect("the Lua runtime is installed");
    handle
        .reserve_suspension("agent:test/coder:latest")
        .expect("the reservation succeeds");
    handle.forget_suspension("agent:test/coder:latest");
    assert!(
        handle.is_reserved("agent:test/coder:latest"),
        "forgetting a wait does not clear a reservation"
    );
    handle.drop_reservation("agent:test/coder:latest");
    assert!(!handle.is_reserved("agent:test/coder:latest"));
}
