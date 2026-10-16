use crate::*;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity};
use margatroid_types::agent_driver_events::{LoadAgentDriver, LoadAgentDriverResult};
use margatroid_types::agent_events::{
    AgentError, AgentErrorKind, AgentStartDefinition, StartAgent, StartAgentResult, StopAgent,
    StopAgentResult,
};
use margatroid_types::resource_components::{AgentDriver, AgentDriverModelConfig};
use resource_id_plugin::{ResourceID, ResourcePlugin, ResourceSet, WorldResourceExt};
use std::sync::Arc;

#[tokio::test]
async fn empty_stop_produces_real_async_thread_exit_receipt_before_despawn() {
    let mut app = app();
    let entity = start(&mut app, 0);
    let (release, wait) = tokio::sync::oneshot::channel();
    app.world()
        .workspace_submit(
            entity,
            Box::new(move || {
                Box::pin(async move {
                    wait.await.unwrap();
                })
            }),
        )
        .unwrap();
    handler::handle_stop_workspace(
        app.world_mut(),
        StopWorkspace {
            id: "stop".into(),
            workspace: entity,
        },
    );
    assert!(app.world().is_alive(entity));
    app.world_mut().advance_event_frame();
    assert!(app
        .world()
        .event_reader::<WorkspaceExecutionCloseOutput>()
        .is_empty());
    release.send(()).unwrap();
    let output = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            app.world_mut().advance_event_frame();
            let output = app
                .world()
                .event_reader::<WorkspaceExecutionCloseOutput>()
                .into_iter()
                .next()
                .cloned();
            if let Some(output) = output {
                break output;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(output.workspace, entity);
    assert_eq!(output.request_id, "stop");
    assert_eq!(output.result, Ok(()));
    handler::handle_workspace_execution_closed(app.world_mut(), output);
    assert!(!app.world().is_alive(entity));
}

#[tokio::test]
async fn agent_exception_checks_owner_deduplicates_and_closes_admission_without_despawn() {
    use margatroid_types::agent_events::AgentExceptionResult;
    use margatroid_types::lua_runtime_components::DriverLuaVmInstance;
    let mut app = app();
    let entity = start(&mut app, 0);
    let agent = app.world_mut().spawn();
    let agent_id = id("agent:local/a");
    app.world_mut()
        .get_component_mut::<Workspace>(entity)
        .unwrap()
        .insert_agent(agent_id.clone(), agent);
    let mut event = AgentExceptionResult {
        exception_id: "exception".into(),
        agent,
        resource_id: agent_id,
        workspace: id("workspace:local/test"),
        instance: DriverLuaVmInstance {
            owner: entity,
            start_request_id: "start-agent".into(),
        },
        error: Some(AgentError::new(AgentErrorKind::CleanupFailed, "original")),
        result: Err(AgentError::new(AgentErrorKind::CleanupFailed, "cleanup")),
        execution_ended: true,
        cleanup_completed: false,
    };
    handler::handle_agent_exception_result(app.world_mut(), event.clone());
    assert!(app
        .world()
        .workspace_submit(entity, Box::new(|| Box::pin(async {})))
        .is_ok());
    event.instance.owner = agent;
    handler::handle_agent_exception_result(app.world_mut(), event.clone());
    handler::handle_agent_exception_result(app.world_mut(), event);
    assert!(app
        .world()
        .workspace_submit(entity, Box::new(|| Box::pin(async {})))
        .is_err());
    assert!(app.world().is_alive(entity));
    assert!(app.world().is_alive(agent));
    app.world_mut().advance_event_frame();
    assert_eq!(
        app.world().event_reader::<WorkspaceExceptionResult>().len(),
        1
    );
    cleanup(&app, entity).await;
}

fn id(value: &str) -> ResourceID {
    ResourceID::parse(value).unwrap()
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(WorkspacePlugin::default());
    app.world_mut()
        .register_resources::<ResourceSet<AgentDriver>>("driver")
        .unwrap();
    app
}
fn definition(members: usize) -> WorkspaceDefinition {
    WorkspaceDefinition {
        id: id("workspace:local/test"),
        name: "test".into(),
        manager: if members > 0 { "a0".into() } else { "".into() },
        project_root: "/project".into(),
        agents: (0..members)
            .map(|n| WorkspaceAgentDefinition {
                id: id(&format!("agent:local/a{n}")),
                name: format!("a{n}"),
                driver: id("driver:local/shared"),
                home_root: "/home/test".into(),
                history_path: format!("/history/a{n}").into(),
            })
            .collect(),
    }
}
fn start(app: &mut App, members: usize) -> Entity {
    handler::handle_start_workspace(
        app.world_mut(),
        StartWorkspace {
            id: "start".into(),
            definition: definition(members),
        },
    );
    app.world().workspace_list()[0]
}
fn install_driver(app: &mut App, entity: Entity) {
    let mut drivers = ResourceSet::new();
    drivers.insert(
        id("driver:local/shared"),
        AgentDriver {
            base_driver: Arc::from(""),
            origin: Arc::new("/driver/driver.lua".into()),
            dependencies: Arc::from([]),
            model: AgentDriverModelConfig {
                model: "test".into(),
                temperature: None,
                max_output_tokens: None,
                top_p: None,
                stop: vec![],
            },
        },
    );
    app.world_mut().insert_resources(entity, drivers).unwrap();
}
async fn cleanup(app: &App, entity: Entity) {
    if let Some(workspace) = app.world().get_component::<Workspace>(entity) {
        workspace.execution_close().close().await.unwrap();
    }
}

#[tokio::test]
async fn empty_workspace_is_running_and_duplicate_start_preserves_original() {
    let mut app = app();
    let entity = start(&mut app, 0);
    assert!(matches!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .state(),
        WorkspaceState::Running
    ));
    assert_eq!(
        app.world()
            .workspace_by_id(&id("workspace:local/test"))
            .unwrap(),
        entity
    );
    handler::handle_start_workspace(
        app.world_mut(),
        StartWorkspace {
            id: "duplicate".into(),
            definition: definition(0),
        },
    );
    app.world_mut().advance_event_frame();
    let results: Vec<_> = app
        .world()
        .event_reader::<StartWorkspaceResult>()
        .into_iter()
        .cloned()
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].result, Ok(entity));
    assert_eq!(
        results[1].result.as_ref().unwrap_err().kind(),
        WorkspaceErrorKind::DuplicateWorkspace
    );
    cleanup(&app, entity).await;
}

#[tokio::test]
async fn startup_waits_for_shared_driver_and_all_members_with_exact_correlation() {
    let mut app = app();
    let entity = start(&mut app, 2);
    app.world_mut().advance_event_frame();
    assert!(app
        .world()
        .event_reader::<StartWorkspaceResult>()
        .is_empty());
    let loads: Vec<_> = app
        .world()
        .event_reader::<LoadAgentDriver>()
        .into_iter()
        .cloned()
        .collect();
    assert_eq!(loads.len(), 1);
    let load = &loads[0];
    install_driver(&mut app, entity);
    let wrong = app.world_mut().spawn();
    handler::handle_agent_driver_result(
        app.world_mut(),
        LoadAgentDriverResult {
            id: load.id.clone(),
            target: wrong,
            resource_id: load.resource_id.clone(),
            result: Ok(()),
        },
    );
    assert!(
        matches!(app.world().get_component::<Workspace>(entity).unwrap().state(), WorkspaceState::Starting(state) if state.pending_drivers.len() == 1)
    );
    handler::handle_agent_driver_result(
        app.world_mut(),
        LoadAgentDriverResult {
            id: load.id.clone(),
            target: entity,
            resource_id: load.resource_id.clone(),
            result: Ok(()),
        },
    );
    app.world_mut().advance_event_frame();
    let starts: Vec<_> = app
        .world()
        .event_reader::<StartAgent>()
        .into_iter()
        .cloned()
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(app
        .world()
        .workspace_start_entry(entity, &starts[0].id)
        .is_ok());
    assert!(app
        .world()
        .workspace_start_entry(entity, "unrelated")
        .is_err());
    let a = app.world_mut().spawn();
    let b = app.world_mut().spawn();
    handler::handle_start_agent_result(
        app.world_mut(),
        StartAgentResult {
            id: starts[0].id.clone(),
            result: Ok(a),
        },
    );
    assert!(matches!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .state(),
        WorkspaceState::Starting(_)
    ));
    handler::handle_start_agent_result(
        app.world_mut(),
        StartAgentResult {
            id: starts[0].id.clone(),
            result: Ok(b),
        },
    );
    handler::handle_start_agent_result(
        app.world_mut(),
        StartAgentResult {
            id: starts[1].id.clone(),
            result: Ok(b),
        },
    );
    assert!(matches!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .state(),
        WorkspaceState::Running
    ));
    assert_eq!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .iter()
            .count(),
        2
    );
    cleanup(&app, entity).await;
}

#[tokio::test]
async fn falsely_successful_driver_result_records_member_failure_without_failing_workspace() {
    let mut app = app();
    let entity = start(&mut app, 1);
    app.world_mut().advance_event_frame();
    let load = app
        .world()
        .event_reader::<LoadAgentDriver>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    handler::handle_agent_driver_result(
        app.world_mut(),
        LoadAgentDriverResult {
            id: load.id,
            target: entity,
            resource_id: load.resource_id,
            result: Ok(()),
        },
    );
    assert!(matches!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .state(),
        WorkspaceState::Running
    ));
    assert!(app.world().workspace_stop("stop", entity).is_ok());
    assert_eq!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .member_failures()
            .len(),
        1
    );
    cleanup(&app, entity).await;
}

#[tokio::test]
async fn stop_waits_for_dynamic_creation_and_all_agent_cleanup_before_join() {
    let mut app = app();
    let entity = start(&mut app, 0);
    let agent = app.world_mut().spawn();
    app.world_mut()
        .get_component_mut::<Workspace>(entity)
        .unwrap()
        .insert_agent(id("agent:local/first"), agent);
    app.world_mut()
        .workspace_accept_agent_start(
            entity,
            StartAgent {
                id: "dynamic".into(),
                resource_id: id("agent:local/dynamic"),
                workspace: entity,
                driver: id("driver:local/shared"),
                definition: AgentStartDefinition {
                    project_root: "/project".into(),
                    home_root: "/home".into(),
                    history_path: "/history".into(),
                },
            },
        )
        .unwrap();
    handler::handle_stop_workspace(
        app.world_mut(),
        StopWorkspace {
            id: "stop".into(),
            workspace: entity,
        },
    );
    assert!(app
        .world()
        .workspace_submit(entity, Box::new(|| Box::pin(async {})))
        .is_err());
    app.world_mut().advance_event_frame();
    let stop = app
        .world()
        .event_reader::<StopAgent>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    handler::handle_stop_agent_result(
        app.world_mut(),
        StopAgentResult {
            id: stop.id.clone(),
            result: Ok(agent),
        },
    );
    assert!(
        matches!(app.world().get_component::<Workspace>(entity).unwrap().state(), WorkspaceState::Stopping(state) if !state.closing_execution)
    );
    let dynamic = app.world_mut().spawn();
    handler::handle_start_agent_result(
        app.world_mut(),
        StartAgentResult {
            id: "dynamic".into(),
            result: Ok(dynamic),
        },
    );
    app.world_mut().advance_event_frame();
    let stop = app
        .world()
        .event_reader::<StopAgent>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    let wrong = app.world_mut().spawn();
    handler::handle_stop_agent_result(
        app.world_mut(),
        StopAgentResult {
            id: stop.id.clone(),
            result: Ok(wrong),
        },
    );
    assert!(
        matches!(app.world().get_component::<Workspace>(entity).unwrap().state(), WorkspaceState::Stopping(state) if state.pending_agents.len() == 1)
    );
    handler::handle_stop_agent_result(
        app.world_mut(),
        StopAgentResult {
            id: stop.id,
            result: Ok(dynamic),
        },
    );
    let close = app
        .world()
        .get_component::<Workspace>(entity)
        .unwrap()
        .execution_close();
    close.close().await.unwrap();
    assert!(app.world().is_alive(entity));
    handler::handle_workspace_execution_closed(
        app.world_mut(),
        WorkspaceExecutionCloseOutput {
            workspace: entity,
            request_id: "old-stop".into(),
            result: Ok(()),
        },
    );
    assert!(app.world().is_alive(entity));
    handler::handle_workspace_execution_closed(
        app.world_mut(),
        WorkspaceExecutionCloseOutput {
            workspace: entity,
            request_id: "stop".into(),
            result: Ok(()),
        },
    );
    assert!(!app.world().is_alive(entity));
    assert!(app.world().is_alive(agent));
    assert!(app.world().is_alive(dynamic));
}

#[tokio::test]
async fn failed_member_stop_preserves_workspace_and_member() {
    let mut app = app();
    let entity = start(&mut app, 0);
    let agent = app.world_mut().spawn();
    app.world_mut()
        .get_component_mut::<Workspace>(entity)
        .unwrap()
        .insert_agent(id("agent:local/first"), agent);
    handler::handle_stop_workspace(
        app.world_mut(),
        StopWorkspace {
            id: "stop".into(),
            workspace: entity,
        },
    );
    app.world_mut().advance_event_frame();
    let stop = app
        .world()
        .event_reader::<StopAgent>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    handler::handle_stop_agent_result(
        app.world_mut(),
        StopAgentResult {
            id: stop.id,
            result: Err(AgentError::new(
                AgentErrorKind::CleanupFailed,
                "cleanup failed",
            )),
        },
    );
    assert!(matches!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .state(),
        WorkspaceState::Failed { .. }
    ));
    assert_eq!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .iter()
            .count(),
        1
    );
    assert!(app.world().is_alive(agent));
    cleanup(&app, entity).await;
}

#[tokio::test]
async fn shared_missing_driver_fans_out_failures_and_all_failed_workspace_can_restart() {
    use margatroid_types::agent_driver_events::DriverLoadError;
    let mut app = app();
    let entity = start(&mut app, 2);
    app.world_mut().advance_event_frame();
    let load = app
        .world()
        .event_reader::<LoadAgentDriver>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    handler::handle_agent_driver_result(
        app.world_mut(),
        LoadAgentDriverResult {
            id: load.id,
            target: entity,
            resource_id: load.resource_id.clone(),
            result: Err(DriverLoadError::new("missing driver source")),
        },
    );
    let workspace = app.world().get_component::<Workspace>(entity).unwrap();
    assert!(matches!(workspace.state(), WorkspaceState::Running));
    assert_eq!(workspace.member_failures().len(), 2);
    assert!(workspace
        .member_failures()
        .values()
        .all(|failure| failure.driver == load.resource_id
            && failure.error.message().contains("missing driver source")));
    assert!(app
        .world()
        .workspace_submit(entity, Box::new(|| Box::pin(async {})))
        .is_ok());
    app.world_mut().advance_event_frame();
    let failures = app.world().event_reader::<WorkspaceMemberStartFailed>();
    assert_eq!(failures.len(), 2);
    assert!(failures
        .into_iter()
        .all(|failure| failure.workspace == entity
            && failure.workspace_id == id("workspace:local/test")));
    assert_eq!(
        app.world()
            .event_reader::<StartWorkspaceResult>()
            .into_iter()
            .next()
            .unwrap()
            .result,
        Ok(entity)
    );
    handler::handle_stop_workspace(
        app.world_mut(),
        StopWorkspace {
            id: "stop".into(),
            workspace: entity,
        },
    );
    cleanup(&app, entity).await;
    handler::handle_workspace_execution_closed(
        app.world_mut(),
        WorkspaceExecutionCloseOutput {
            workspace: entity,
            request_id: "stop".into(),
            result: Ok(()),
        },
    );
    assert!(!app.world().is_alive(entity));
    let restarted = start(&mut app, 0);
    assert_ne!(restarted, entity);
    assert!(app
        .world()
        .get_component::<Workspace>(restarted)
        .unwrap()
        .member_failures()
        .is_empty());
    cleanup(&app, restarted).await;
}

#[tokio::test]
async fn member_failure_does_not_block_healthy_member_or_dynamic_stop() {
    let mut app = app();
    let entity = start(&mut app, 2);
    app.world_mut().advance_event_frame();
    let load = app
        .world()
        .event_reader::<LoadAgentDriver>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    install_driver(&mut app, entity);
    handler::handle_agent_driver_result(
        app.world_mut(),
        LoadAgentDriverResult {
            id: load.id,
            target: entity,
            resource_id: load.resource_id,
            result: Ok(()),
        },
    );
    app.world_mut().advance_event_frame();
    let starts: Vec<_> = app
        .world()
        .event_reader::<StartAgent>()
        .into_iter()
        .cloned()
        .collect();
    let healthy = app.world_mut().spawn();
    handler::handle_start_agent_result(
        app.world_mut(),
        StartAgentResult {
            id: starts[0].id.clone(),
            result: Ok(healthy),
        },
    );
    handler::handle_start_agent_result(
        app.world_mut(),
        StartAgentResult {
            id: starts[1].id.clone(),
            result: Err(AgentError::new(
                AgentErrorKind::CleanupFailed,
                "member startup detail",
            )),
        },
    );
    let workspace = app.world().get_component::<Workspace>(entity).unwrap();
    assert!(matches!(workspace.state(), WorkspaceState::Running));
    assert_eq!(workspace.iter().count(), 1);
    assert_eq!(workspace.member_failures().len(), 1);
    app.world_mut()
        .workspace_accept_agent_start(
            entity,
            StartAgent {
                id: "dynamic-fail".into(),
                resource_id: id("agent:local/dynamic-fail"),
                workspace: entity,
                driver: id("driver:local/dynamic"),
                definition: AgentStartDefinition {
                    project_root: "/project".into(),
                    home_root: "/home".into(),
                    history_path: "/history".into(),
                },
            },
        )
        .unwrap();
    handler::handle_stop_workspace(
        app.world_mut(),
        StopWorkspace {
            id: "stop".into(),
            workspace: entity,
        },
    );
    handler::handle_start_agent_result(
        app.world_mut(),
        StartAgentResult {
            id: "dynamic-fail".into(),
            result: Err(AgentError::new(
                AgentErrorKind::CleanupFailed,
                "dynamic creation rejected",
            )),
        },
    );
    let workspace = app.world().get_component::<Workspace>(entity).unwrap();
    assert_eq!(workspace.member_failures().len(), 2);
    assert!(
        matches!(workspace.state(), WorkspaceState::Stopping(state) if state.error.is_none() && state.pending_creations.is_empty())
    );
    app.world_mut().advance_event_frame();
    let stop = app
        .world()
        .event_reader::<StopAgent>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    handler::handle_stop_agent_result(
        app.world_mut(),
        StopAgentResult {
            id: stop.id,
            result: Ok(healthy),
        },
    );
    cleanup(&app, entity).await;
    handler::handle_workspace_execution_closed(
        app.world_mut(),
        WorkspaceExecutionCloseOutput {
            workspace: entity,
            request_id: "stop".into(),
            result: Ok(()),
        },
    );
    assert!(!app.world().is_alive(entity));
}

#[tokio::test]
async fn healthy_and_missing_driver_finish_running_with_full_failure_context() {
    use margatroid_types::agent_driver_events::DriverLoadError;
    let mut app = app();
    let mut definition = definition(2);
    definition.agents[1].driver = id("driver:local/missing");
    handler::handle_start_workspace(
        app.world_mut(),
        StartWorkspace {
            id: "mixed".into(),
            definition,
        },
    );
    let entity = app.world().workspace_list()[0];
    app.world_mut().advance_event_frame();
    let loads: Vec<_> = app
        .world()
        .event_reader::<LoadAgentDriver>()
        .into_iter()
        .cloned()
        .collect();
    install_driver(&mut app, entity);
    for load in loads {
        let result = if load.resource_id == id("driver:local/missing") {
            Err(DriverLoadError::new(
                "missing manifest: /drivers/missing/driver.toml",
            ))
        } else {
            Ok(())
        };
        handler::handle_agent_driver_result(
            app.world_mut(),
            LoadAgentDriverResult {
                id: load.id,
                target: entity,
                resource_id: load.resource_id,
                result,
            },
        );
    }
    assert!(matches!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .state(),
        WorkspaceState::Starting(_)
    ));
    app.world_mut().advance_event_frame();
    let request = app
        .world()
        .event_reader::<StartAgent>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    let healthy = app.world_mut().spawn();
    handler::handle_start_agent_result(
        app.world_mut(),
        StartAgentResult {
            id: request.id,
            result: Ok(healthy),
        },
    );
    let workspace = app.world().get_component::<Workspace>(entity).unwrap();
    assert!(matches!(workspace.state(), WorkspaceState::Running));
    assert_eq!(workspace.iter().count(), 1);
    let failure = &workspace.member_failures()[&id("agent:local/a1")];
    assert_eq!(failure.driver, id("driver:local/missing"));
    assert!(failure
        .error
        .message()
        .contains("/drivers/missing/driver.toml"));
    assert!(app
        .world()
        .workspace_submit(entity, Box::new(|| Box::pin(async {})))
        .is_ok());
    cleanup(&app, entity).await;
}

#[tokio::test]
async fn precreation_and_duplicate_errors_are_distinct_and_do_not_replace_instances() {
    let mut app = app();
    let mut invalid = definition(0);
    invalid.project_root = "".into();
    handler::handle_start_workspace(
        app.world_mut(),
        StartWorkspace {
            id: "invalid".into(),
            definition: invalid.clone(),
        },
    );
    assert!(app.world().workspace_list().is_empty());
    assert!(id("workspace:local/test")
        .find_entities(app.world())
        .unwrap()
        .is_empty());
    let entity = start(&mut app, 0);
    handler::handle_start_workspace(
        app.world_mut(),
        StartWorkspace {
            id: "duplicate".into(),
            definition: definition(0),
        },
    );
    app.world_mut().advance_event_frame();
    let results: Vec<_> = app
        .world()
        .event_reader::<StartWorkspaceResult>()
        .into_iter()
        .cloned()
        .collect();
    let invalid = results
        .iter()
        .find(|result| result.id == "invalid")
        .unwrap()
        .result
        .as_ref()
        .unwrap_err();
    let duplicate = results
        .iter()
        .find(|result| result.id == "duplicate")
        .unwrap()
        .result
        .as_ref()
        .unwrap_err();
    assert_eq!(invalid.kind(), WorkspaceErrorKind::InvalidDefinition);
    assert_eq!(duplicate.kind(), WorkspaceErrorKind::DuplicateWorkspace);
    for context in [
        "workspace:local/test:latest",
        "state=Running",
        "members=0",
        "member_failures=0",
    ] {
        assert!(
            duplicate.message().contains(context),
            "{}",
            duplicate.message()
        );
    }
    assert_eq!(app.world().workspace_list(), vec![entity]);
    cleanup(&app, entity).await;
}
