use std::sync::{Arc, Mutex};

use agent_core::{Agent, AgentState};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, World};
use serde_json::json;
use tokio::sync::oneshot;
use tool_core::{ToolExecuteRequest, ToolTasks};

use crate::{handler, ToolPlugin};

#[test]
fn admission_rechecks_current_turn_and_never_registers_rejected_requests() {
    for (state, current, requested) in [
        (AgentState::Working, None, "turn"),
        (AgentState::Working, Some(""), ""),
        (AgentState::Working, Some("new-turn"), "old-turn"),
        (AgentState::Idle, Some("turn"), "turn"),
        (AgentState::Starting, Some("turn"), "turn"),
        (AgentState::Restarting, Some("turn"), "turn"),
        (AgentState::Stopping, Some("turn"), "turn"),
    ] {
        let mut world = World::new();
        let entity = world.spawn();
        let tasks = ToolTasks::new();
        world.insert_component(entity, tasks.clone());
        world.insert_component(
            entity,
            Agent {
                name: "agent".into(),
                workspace: entity,
                state,
                turn: current.map(str::to_owned),
            },
        );
        let (reply, mut result) = oneshot::channel();
        handler::handle_tool_execute(
            &mut world,
            ToolExecuteRequest {
                id: "task".into(),
                agent: entity,
                turn: requested.into(),
                call_id: "original-call".into(),
                alias: "original-alias".into(),
                arguments: json!({}),
                reply: Arc::new(Mutex::new(Some(reply))),
            },
        );
        let response = result.try_recv().unwrap().unwrap();
        assert!(response.failed);
        assert_eq!(response.content, "Tool call aborted.");
        assert_eq!(response.call_id, "original-call");
        assert_eq!(response.alias, "original-alias");
        assert!(tasks.list().is_empty());
    }
}

#[test]
fn missing_agent_returns_an_aborted_correlated_result() {
    let mut world = World::new();
    let entity = world.spawn();
    let (reply, mut result) = oneshot::channel();
    handler::handle_tool_execute(
        &mut world,
        ToolExecuteRequest {
            id: "task".into(),
            agent: entity,
            turn: "turn".into(),
            call_id: "call".into(),
            alias: "tool".into(),
            arguments: json!({}),
            reply: Arc::new(Mutex::new(Some(reply))),
        },
    );
    let response = result.try_recv().unwrap().unwrap();
    assert!(response.failed);
    assert_eq!(
        (response.call_id.as_str(), response.alias.as_str()),
        ("call", "tool")
    );
    assert!(!world.contains_component::<ToolTasks>(entity));
}

#[test]
fn installation_accepts_only_schedules_after_abort_processing() {
    for schedule in [
        RuntimePlugin::UPDATE,
        RuntimePlugin::POST_UPDATE,
        RuntimePlugin::PRE_UPDATE,
        RuntimePlugin::STARTUP,
        "custom",
    ] {
        let installed = std::panic::catch_unwind(|| {
            let mut app = App::new();
            app.add_plugin(RuntimePlugin::default())
                .add_plugin(AsyncRuntimePlugin)
                .add_plugin(ClosurePlugin);
            if schedule == "custom" {
                app.add_schedule(schedule.to_owned());
            }
            app.add_closure_system(schedule);
            app.add_plugin(ToolPlugin::new().with_schedule(schedule));
        });
        assert_eq!(
            installed.is_ok(),
            matches!(schedule, RuntimePlugin::UPDATE | RuntimePlugin::POST_UPDATE),
            "schedule: {schedule}"
        );
    }
}
