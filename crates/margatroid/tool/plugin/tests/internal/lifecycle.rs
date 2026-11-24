#[path = "../support/storage.rs"]
mod storage_fixture;
use crate::test_support;

use std::sync::{Arc, Mutex};

use agent_core::AgentState;
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
        (AgentState::Running, None, "turn"),
        (AgentState::Running, Some(""), ""),
        (AgentState::Running, Some("new-turn"), "old-turn"),
        (AgentState::Closing, Some("turn"), "turn"),
        (AgentState::Starting, Some("turn"), "turn"),
        (AgentState::Closing, Some("turn"), "turn"),
    ] {
        let mut world = World::new();
        let entity = world.spawn();
        let tasks = test_tasks();
        world.insert_component(entity, tasks.clone());
        world.insert_component(
            entity,
            test_support::agent(
                "agent".into(),
                entity,
                state,
                current.map(str::to_owned),
                test_agent_context(),
            ),
        );
        let (reply, mut result) = oneshot::channel();
        handler::handle_tool_execute(
            &mut world,
            ToolExecuteRequest::Registered {
                id: "task".into(),
                agent: entity,
                turn: Some(requested.into()),
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
        ToolExecuteRequest::Registered {
            id: "task".into(),
            agent: entity,
            turn: Some("turn".into()),
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

#[test]
fn dynamic_admission_checks_schema_without_rechecking_recorded_owner() {
    use tool_core::{ToolErrorKind, ToolExecutionLimits, ToolTaskHandle};
    use tool_resource_core::{ToolEntry, Tools};
    use workspace_core::WorkspaceState;

    for (arguments, cancelled_owner, expected) in [
        (json!({}), false, Some(ToolErrorKind::InvalidArguments)),
        (json!({"value": 1}), false, None),
        (json!({"value": 1}), true, None),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(ToolPlugin::new());
        let agent = app.world_mut().spawn();
        storage_fixture::mount(app.world_mut(), agent, root.path());
        app.world_mut().insert_component(
            agent,
            test_support::agent(
                "agent".into(),
                agent,
                AgentState::Running,
                Some("turn".into()),
                test_agent_context(),
            ),
        );
        app.world_mut().insert_component(
            agent,
            test_support::workspace(
                "workspace".into(),
                root.path().join("workspace.yaml"),
                root.path().to_owned(),
                WorkspaceState::Running,
            ),
        );
        let tasks = test_tasks();
        if cancelled_owner {
            let (reply, _receiver) = oneshot::channel();
            let (owner, _input) = ToolTaskHandle::new(
                &ToolExecutionLimits::default(),
                "owner-call".into(),
                "factory".into(),
                reply,
                tasks.update_context(),
                None,
                test_support::finish(),
            )
            .unwrap();
            owner.cancel();
            tasks.register("owner".into(), owner).unwrap();
        }
        app.world_mut().insert_component(agent, tasks.clone());
        let mut tools = Tools::new();
        tools
            .insert(
                "dynamic".into(),
                ToolEntry {
                    owner_agent: None,
                    path: root.path().join("tool.toml"),
                    description: "dynamic tool".into(),
                    input_schema: json!({"type":"object","required":["value"]}),
                    owner_task_id: Some("owner".into()),
                    binding: Some(json!({"operation":"call"})),
                },
            )
            .unwrap();
        app.world_mut().insert_component(agent, tools);
        let (reply, mut result) = oneshot::channel();
        handler::handle_tool_execute(
            app.world_mut(),
            ToolExecuteRequest::Registered {
                id: "dependent".into(),
                agent,
                turn: Some("turn".into()),
                alias: "dynamic".into(),
                call_id: "call".into(),
                arguments,
                reply: Arc::new(Mutex::new(Some(reply))),
            },
        );
        if let Some(expected) = expected {
            assert_eq!(result.try_recv().unwrap().unwrap_err().kind, expected);
            assert!(tasks.get("dependent").is_err());
            assert_eq!(tasks.list().len(), usize::from(cancelled_owner));
        } else {
            assert!(matches!(
                result.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            assert!(tasks.get("dependent").is_ok());
            assert_eq!(tasks.list().len(), 1 + usize::from(cancelled_owner));
            tasks.cancel("dependent").unwrap();
            let handle = tasks.get("dependent").unwrap();
            // Admission only queued launch; no worker or child exists yet.
            assert!(tasks.complete_cleanup("dependent", &handle, None));
            assert!(tasks.get("dependent").is_err());
        }
    }
}

fn test_tasks() -> tool_core::ToolTasks {
    let mut app = core_plugin::App::new();
    let _owner = app.world_mut().spawn();
    tool_core::ToolTasks::new()
}

fn test_agent_context() -> agent_core::AgentUpdateContext {
    agent_core::AgentUpdateContext::new(|_, _, _| Vec::new(), |_, _| {})
}
