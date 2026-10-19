use agent_plugin::*;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity, Event, World};
use margatroid_types::{
    execution_components::ExecutionActivityKind,
    lua_runtime_components::{
        DriverCloseHandle, DriverLuaVmInstance, LuaRuntimeError, LuaRuntimeErrorKind,
    },
    lua_runtime_events::*,
    resource_components::*,
    workspace_components::*,
    workspace_execution::spawn_workspace_execution,
};
use resource_id_plugin::{ResourceID, ResourcePlugin, ResourceSet, WorldResourceExt};
use std::{collections::BTreeMap, sync::Arc};

fn id(kind: &str, name: &str) -> ResourceID {
    ResourceID::new(kind, "test", name, None).unwrap()
}
fn events<E: Event + Clone>(world: &mut World) -> Vec<E> {
    world.advance_event_frame();
    world.event_reader::<E>().into_iter().cloned().collect()
}
struct Fixture {
    app: App,
    request: StartAgent,
    close: margatroid_types::workspace_execution::WorkspaceExecutionClose,
    _temp: tempfile::TempDir,
}
fn test_catalog(
    context_window_tokens: u64,
) -> margatroid_types::model_components::ModelRuntimeCatalog {
    use margatroid_types::model_components::*;
    ModelRuntimeCatalog::new([ModelRuntimeInfo {
        id: "model-one".into(),
        model: "upstream-model".into(),
        api_type: "chat_completions".into(),
        provider: Some("test-provider".into()),
        context_window_tokens,
        thinking: Some("enabled".into()),
        reasoning_effort: Some("high".into()),
    }])
}
fn fixture() -> Fixture {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ResourcePlugin)
        .add_plugin(AsyncRuntimePlugin::default());
    let world = app.world_mut();
    world.insert_resource(test_catalog(128_000));
    world.insert_resource(margatroid_types::mcl_environment::MclEnvironmentFactory(
        Arc::new(|| margatroid_types::lua_environment::LuaEnvironment::default()),
    ));
    world.register_library::<Workspace>("workspace").unwrap();
    world
        .register_resources::<ResourceSet<AgentDriver>>("driver")
        .unwrap();
    world
        .register_resources::<ResourceSet<ToolResource>>("tool")
        .unwrap();
    world
        .register_resources::<ResourceSet<PromptResource>>("prompt")
        .unwrap();
    world
        .register_resources::<ResourceSet<SandboxResource>>("sandbox")
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let workspace = world.spawn();
    let request = StartAgent {
        id: "original".into(),
        resource_id: id("agent", "one"),
        workspace,
        driver: id("driver", "base"),
        definition: AgentStartDefinition {
            project_root: temp.path().into(),
            home_root: temp.path().join("home"),
            history_path: temp.path().join("history.sqlite"),
        },
    };
    let (entry, close) = spawn_workspace_execution().unwrap();
    let definition = WorkspaceDefinition {
        id: id("workspace", "one"),
        name: "one".into(),
        project_root: temp.path().into(),
        agents: vec![],
    };
    world
        .insert_library(
            workspace,
            Workspace::new(
                definition,
                WorkspaceState::Starting(WorkspaceStartState {
                    request_id: "workspace-start".into(),
                    pending_drivers: BTreeMap::new(),
                    driver_members: BTreeMap::new(),
                    pending_agents: BTreeMap::from([(
                        request.id.clone(),
                        request.resource_id.clone(),
                    )]),
                    error: None,
                }),
                entry,
                close.clone(),
            ),
        )
        .unwrap();
    let mut drivers = ResourceSet::new();
    drivers.insert(
        request.driver.clone(),
        AgentDriver {
            base_driver: Arc::from("ready()"),
            origin: Arc::new(temp.path().join("driver.lua")),
            dependencies: Arc::from([id("tool", "not-preloaded")]),
            model: AgentDriverModelConfig {
                model: "model-one".into(),
                temperature: Some(0.2),
                max_output_tokens: None,
                top_p: None,
                stop: vec![],
            },
        },
    );
    world.insert_resources(workspace, drivers).unwrap();
    app.add_plugin(AgentPlugin::default());
    Fixture {
        app,
        request,
        close,
        _temp: temp,
    }
}
async fn database_result(f: &mut Fixture) -> Result<DatabaseSetupOutput, DatabaseSetupError> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let mut results =
                events::<Result<DatabaseSetupOutput, DatabaseSetupError>>(f.app.world_mut());
            if let Some(result) = results.pop() {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
async fn start(f: &mut Fixture) -> (Entity, StartDriverLuaVm) {
    handle_start_agent(f.app.world_mut(), f.request.clone());
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
    assert!(events::<StartDriverLuaVm>(f.app.world_mut()).is_empty());
    let result = database_result(f).await;
    handle_database_setup_result(f.app.world_mut(), result);
    let requests = events::<StartDriverLuaVm>(f.app.world_mut());
    assert_eq!(requests.len(), 1);
    (requests[0].owner, requests[0].clone())
}
fn make_running(
    f: &mut Fixture,
    entity: Entity,
    request: &StartDriverLuaVm,
) -> DriverLuaVmInstance {
    let instance = DriverLuaVmInstance {
        owner: entity,
        start_request_id: request.id.clone(),
    };
    let agent = f
        .app
        .world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap();
    agent.lua.instance = Some(instance.clone());
    agent.lua.close = Some(DriverCloseHandle::new(instance.clone()));
    handle_start_driver_lua_vm_result(
        f.app.world_mut(),
        StartDriverLuaVmResult {
            id: request.id.clone(),
            owner: entity,
            result: Ok(()),
        },
    );
    instance
}

#[tokio::test]
async fn model_environment_is_complete_private_and_an_admission_snapshot() {
    use margatroid_types::lua_environment::{LuaBindingValue, LuaValue};
    let mut f = fixture();
    handle_start_agent(f.app.world_mut(), f.request.clone());
    f.app.world_mut().insert_resource(test_catalog(256_000));
    let result = database_result(&mut f).await;
    handle_database_setup_result(f.app.world_mut(), result);
    let requests = events::<StartDriverLuaVm>(f.app.world_mut());
    let binding = &requests[0]
        .environment
        .globals
        .iter()
        .find(|g| g.name == "agent_info")
        .unwrap()
        .binding;
    let LuaBindingValue::Value(LuaValue::Object(info)) = binding else {
        panic!("agent_info object missing")
    };
    assert_eq!(
        info["driver"],
        LuaValue::String(f.request.driver.to_string())
    );
    assert_eq!(
        info["driver_dependencies"],
        LuaValue::Array(vec![LuaValue::String(
            id("tool", "not-preloaded").to_string()
        )])
    );
    assert!(!info.contains_key("image"));
    assert!(!info.contains_key("image_dependencies"));
    let LuaValue::Object(model) = &info["model"] else {
        panic!("model object missing")
    };
    assert_eq!(model["model"], LuaValue::String("model-one".into()));
    assert_eq!(
        model["provider_model"],
        LuaValue::String("upstream-model".into())
    );
    assert_eq!(model["context_window_tokens"], LuaValue::Integer(128_000));
    assert_eq!(
        f.app
            .world()
            .get_component::<margatroid_types::model_components::ModelRuntimeInfo>(
                requests[0].owner
            )
            .unwrap()
            .context_window_tokens,
        128_000
    );
    assert_eq!(
        model["api_type"],
        LuaValue::String("chat_completions".into())
    );
    assert_eq!(model["provider"], LuaValue::String("test-provider".into()));
    assert_eq!(model["thinking"], LuaValue::String("enabled".into()));
    assert_eq!(model["reasoning_effort"], LuaValue::String("high".into()));
    assert_eq!(model["temperature"], LuaValue::Number(0.2_f32 as f64));
    assert_eq!(model["max_output_tokens"], LuaValue::Nil);
    assert_eq!(model["top_p"], LuaValue::Nil);
    assert_eq!(model["stop"], LuaValue::Array(vec![]));
    assert_eq!(model.len(), 11);
    for field in ["api_key", "base_url", "headers", "authorization"] {
        assert!(!model.contains_key(field));
    }
    assert_eq!(
        f.app
            .world()
            .get_resource::<margatroid_types::model_components::ModelRuntimeCatalog>()
            .unwrap()
            .get("model-one")
            .unwrap()
            .context_window_tokens,
        256_000
    );
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn missing_catalog_rejects_without_inventing_context_window() {
    let mut f = fixture();
    f.app
        .world_mut()
        .remove_resource::<margatroid_types::model_components::ModelRuntimeCatalog>();
    handle_start_agent(f.app.world_mut(), f.request.clone());
    let results = events::<StartAgentResult>(f.app.world_mut());
    assert_eq!(
        results[0].result.as_ref().unwrap_err().kind,
        AgentErrorKind::ModelRouteNotFound
    );
    assert!(!f.request.definition.history_path.exists());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn missing_model_route_rejects_before_database_or_vm_creation() {
    let mut f = fixture();
    f.app
        .world_mut()
        .insert_resource(margatroid_types::model_components::ModelRuntimeCatalog::default());
    handle_start_agent(f.app.world_mut(), f.request.clone());
    let results = events::<StartAgentResult>(f.app.world_mut());
    assert_eq!(
        results[0].result.as_ref().unwrap_err().kind,
        AgentErrorKind::ModelRouteNotFound
    );
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
    assert!(events::<StartDriverLuaVm>(f.app.world_mut()).is_empty());
    assert!(!f.request.definition.history_path.exists());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn start_waits_for_ready_and_keeps_resources_empty() {
    let mut f = fixture();
    let (entity, request) = start(&mut f).await;
    let world = f.app.world();
    let agent = world.get_component::<Agent>(entity).unwrap();
    assert!(matches!(agent.state, AgentState::Creating { .. }));
    assert_eq!(agent.info.model.model, "model-one");
    assert_eq!(agent.info.driver_dependencies.len(), 1);
    assert!(agent.history.list().unwrap().is_empty());
    assert!(world
        .get_component::<ResourceSet<ToolResource>>(entity)
        .unwrap()
        .is_empty());
    assert!(world
        .get_component::<ResourceSet<PromptResource>>(entity)
        .unwrap()
        .is_empty());
    assert!(world
        .get_component::<ResourceSet<SandboxResource>>(entity)
        .unwrap()
        .is_empty());
    assert!(request
        .environment
        .globals
        .iter()
        .any(|g| g.name == "agent_info"));
    handle_start_driver_lua_vm_result(
        f.app.world_mut(),
        StartDriverLuaVmResult {
            id: "stale".into(),
            owner: entity,
            result: Ok(()),
        },
    );
    assert!(matches!(
        f.app.world().get_component::<Agent>(entity).unwrap().state,
        AgentState::Creating { .. }
    ));
    make_running(&mut f, entity, &request);
    let replies = events::<StartAgentResult>(f.app.world_mut());
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].id, "original");
    assert_eq!(replies[0].result, Ok(entity));
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn failure_requires_cleanup_proof_before_despawn() {
    for kind in [
        LuaRuntimeErrorKind::CompileFailed,
        LuaRuntimeErrorKind::CleanupFailed,
    ] {
        let mut f = fixture();
        let (entity, request) = start(&mut f).await;
        handle_start_driver_lua_vm_result(
            f.app.world_mut(),
            StartDriverLuaVmResult {
                id: request.id,
                owner: entity,
                result: Err(LuaRuntimeError::new(kind, "failed")),
            },
        );
        assert_eq!(
            f.app.world().is_alive(entity),
            kind == LuaRuntimeErrorKind::CleanupFailed
        );
        if let Some(agent) = f.app.world().get_component::<Agent>(entity) {
            assert!(matches!(
                agent.state,
                AgentState::Stopping {
                    vm_request_id: None,
                    ..
                }
            ));
            assert!(agent.execution.is_closed());
            assert!(!agent.mcl.accepting);
        }
        assert!(events::<StartAgentResult>(f.app.world_mut())[0]
            .result
            .is_err());
        f.close.close().await.unwrap();
    }
}

#[tokio::test]
async fn stop_closes_admission_coalesces_waiters_and_requires_matching_result() {
    let mut f = fixture();
    let (entity, request) = start(&mut f).await;
    handle_stop_agent(
        f.app.world_mut(),
        StopAgent {
            id: "too-early".into(),
            agent: entity,
        },
    );
    assert!(events::<StopAgentResult>(f.app.world_mut())[0]
        .result
        .is_err());
    let instance = make_running(&mut f, entity, &request);
    let activity = f
        .app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution
        .register(ExecutionActivityKind::Other)
        .unwrap();
    handle_stop_agent(
        f.app.world_mut(),
        StopAgent {
            id: "stop-1".into(),
            agent: entity,
        },
    );
    handle_stop_agent(
        f.app.world_mut(),
        StopAgent {
            id: "stop-2".into(),
            agent: entity,
        },
    );
    let stop = events::<StopDriverLuaVm>(f.app.world_mut());
    assert_eq!(stop.len(), 1);
    assert!(activity.cancel.is_cancelled());
    assert!(f.app.world().is_alive(entity));
    handle_stop_driver_lua_vm_result(
        f.app.world_mut(),
        StopDriverLuaVmResult {
            id: "stale".into(),
            owner: entity,
            instance: instance.clone(),
            result: Ok(()),
        },
    );
    assert!(f.app.world().is_alive(entity));
    activity.mark_completed().unwrap();
    handle_stop_driver_lua_vm_result(
        f.app.world_mut(),
        StopDriverLuaVmResult {
            id: stop[0].id.clone(),
            owner: entity,
            instance,
            result: Ok(()),
        },
    );
    assert!(!f.app.world().is_alive(entity));
    let replies = events::<StopAgentResult>(f.app.world_mut());
    assert_eq!(replies.len(), 2);
    assert!(replies.iter().all(|reply| reply.result == Ok(entity)));
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn abort_validates_identity_is_idempotent_and_never_closes_agent() {
    let mut f = fixture();
    let (entity, start) = start(&mut f).await;
    let instance = make_running(&mut f, entity, &start);
    let agent = f
        .app
        .world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap();
    let turn = agent.new_turn();
    agent.state = AgentState::Working;
    let activity = agent
        .execution
        .register_for_turn(ExecutionActivityKind::Inference, Some(turn.clone()))
        .unwrap();
    let request = AbortAgentTurn {
        id: "abort".into(),
        agent: entity,
        resource_id: agent.resourceid.clone(),
        workspace: agent.workspace.clone(),
        instance,
        expected_turn_id: turn,
    };
    let mut stale = request.clone();
    stale.instance.start_request_id = "stale".into();
    handle_abort_agent_turn(f.app.world_mut(), stale);
    assert!(events::<AbortAgentTurnResult>(f.app.world_mut())[0]
        .result
        .is_err());
    assert!(!activity.turn_cancel.is_cancelled());
    handle_abort_agent_turn(f.app.world_mut(), request.clone());
    handle_abort_agent_turn(f.app.world_mut(), request.clone());
    let results = events::<AbortAgentTurnResult>(f.app.world_mut());
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.result.is_ok()));
    let agent = f.app.world().get_component::<Agent>(entity).unwrap();
    assert!(agent.turn.is_none());
    assert!(matches!(agent.state, AgentState::Working));
    assert!(!agent.execution.is_closed());
    assert!(agent.mcl.accepting);
    assert!(activity.turn_cancel.is_cancelled());
    assert!(!activity.cancel.is_cancelled());
    activity.mark_completed().unwrap();
    let agent = f
        .app
        .world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap();
    agent.new_turn();
    handle_abort_agent_turn(f.app.world_mut(), request);
    assert!(events::<AbortAgentTurnResult>(f.app.world_mut())[0]
        .result
        .is_err());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn driver_exception_maps_once_without_inventing_original_error() {
    let mut f = fixture();
    let (entity, request) = start(&mut f).await;
    let instance = make_running(&mut f, entity, &request);
    f.app
        .world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .new_turn();
    let event = DriverLuaVmExceptionResult {
        exception_id: "exception".into(),
        instance,
        error: None,
        result: Ok(()),
        execution_ended: true,
        cleanup_completed: true,
    };
    handle_driver_lua_vm_exception_result(f.app.world_mut(), event.clone());
    handle_driver_lua_vm_exception_result(f.app.world_mut(), event);
    let replies = events::<AgentExceptionResult>(f.app.world_mut());
    assert_eq!(replies.len(), 1);
    assert!(replies[0].error.is_none());
    assert!(replies[0].cleanup_completed);
    assert!(f.app.world().is_alive(entity));
    let agent = f.app.world().get_component::<Agent>(entity).unwrap();
    assert!(matches!(
        agent.state,
        AgentState::Stopping {
            vm_request_id: None,
            ..
        }
    ));
    assert!(agent.turn.is_none());
    assert!(agent.execution.is_closed());
    assert!(!agent.mcl.accepting);
    assert_eq!(
        f.app
            .world()
            .event_reader::<AgentException>()
            .into_iter()
            .count(),
        0
    );
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn exception_before_ready_preserves_start_result_correlation() {
    let mut f = fixture();
    let (entity, request) = start(&mut f).await;
    let instance = DriverLuaVmInstance {
        owner: entity,
        start_request_id: request.id.clone(),
    };
    f.app
        .world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .lua
        .instance = Some(instance.clone());
    handle_driver_lua_vm_exception_result(
        f.app.world_mut(),
        DriverLuaVmExceptionResult {
            exception_id: "before-ready".into(),
            instance,
            error: Some(LuaRuntimeError::new(
                LuaRuntimeErrorKind::ExecutionFailed,
                "driver failed before ready",
            )),
            result: Ok(()),
            execution_ended: true,
            cleanup_completed: true,
        },
    );
    let agent = f.app.world().get_component::<Agent>(entity).unwrap();
    assert!(
        matches!(&agent.state, AgentState::Creating { request_id, vm_request_id }
        if request_id == &f.request.id && vm_request_id == &request.id)
    );
    assert!(agent.execution.is_closed());
    handle_start_driver_lua_vm_result(
        f.app.world_mut(),
        StartDriverLuaVmResult {
            id: request.id,
            owner: entity,
            result: Err(LuaRuntimeError::new(
                LuaRuntimeErrorKind::ExecutionFailed,
                "driver failed before ready",
            )),
        },
    );
    assert!(!f.app.world().is_alive(entity));
    let results = events::<StartAgentResult>(f.app.world_mut());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, f.request.id);
    assert!(results[0].result.is_err());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn stale_exception_cannot_close_agent_or_clear_turn() {
    let mut f = fixture();
    let (entity, request) = start(&mut f).await;
    let mut instance = make_running(&mut f, entity, &request);
    let turn = f
        .app
        .world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .new_turn();
    instance.start_request_id = "stale".into();
    let mut event = DriverLuaVmExceptionResult {
        exception_id: "exception".into(),
        instance,
        error: None,
        result: Ok(()),
        execution_ended: true,
        cleanup_completed: true,
    };
    handle_driver_lua_vm_exception_result(f.app.world_mut(), event.clone());
    let agent = f.app.world().get_component::<Agent>(entity).unwrap();
    assert!(matches!(agent.state, AgentState::Idle));
    assert_eq!(agent.turn.as_ref(), Some(&turn));
    assert!(!agent.execution.is_closed());
    assert!(events::<AgentExceptionResult>(f.app.world_mut()).is_empty());
    event.instance.start_request_id = request.id;
    event.execution_ended = false;
    event.cleanup_completed = false;
    handle_driver_lua_vm_exception_result(f.app.world_mut(), event);
    let agent = f.app.world().get_component::<Agent>(entity).unwrap();
    assert!(matches!(agent.state, AgentState::Stopping { .. }));
    assert_eq!(agent.turn.as_ref(), Some(&turn));
    assert!(agent.execution.is_closed());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn driver_exception_preserves_failure_and_pending_stop_completion() {
    use margatroid_types::agent_components::AgentFailure;
    let mut f = fixture();
    let (entity, request) = start(&mut f).await;
    let instance = make_running(&mut f, entity, &request);
    f.app
        .world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .new_turn();
    handle_stop_agent(
        f.app.world_mut(),
        StopAgent {
            id: "stop".into(),
            agent: entity,
        },
    );
    let stop = events::<StopDriverLuaVm>(f.app.world_mut()).pop().unwrap();
    handle_driver_lua_vm_exception_result(
        f.app.world_mut(),
        DriverLuaVmExceptionResult {
            exception_id: "failed".into(),
            instance: instance.clone(),
            error: Some(LuaRuntimeError::new(
                LuaRuntimeErrorKind::ExecutionFailed,
                "tool response missing",
            )),
            result: Ok(()),
            execution_ended: true,
            cleanup_completed: false,
        },
    );
    let replies = events::<AgentExceptionResult>(f.app.world_mut());
    assert_eq!(replies.len(), 1);
    let failure = replies[0].error.as_ref().unwrap();
    assert!(failure.message.contains("tool response missing"));
    assert_eq!(
        &f.app
            .world()
            .get_component::<AgentFailure>(entity)
            .unwrap()
            .0,
        failure
    );
    let agent = f.app.world().get_component::<Agent>(entity).unwrap();
    assert!(agent.turn.is_none());
    assert!(
        matches!(&agent.state, AgentState::Stopping { request_ids, vm_request_id }
        if request_ids == &["stop"] && vm_request_id.as_ref() == Some(&stop.id))
    );
    handle_stop_driver_lua_vm_result(
        f.app.world_mut(),
        StopDriverLuaVmResult {
            id: stop.id,
            owner: entity,
            instance,
            result: Ok(()),
        },
    );
    assert!(!f.app.world().is_alive(entity));
    assert_eq!(
        events::<StopAgentResult>(f.app.world_mut())[0].result,
        Ok(entity)
    );
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn driver_lookup_never_uses_another_entity() {
    let mut f = fixture();
    f.app
        .world_mut()
        .get_component_mut::<ResourceSet<AgentDriver>>(f.request.workspace)
        .unwrap()
        .remove(&f.request.driver);
    handle_start_agent(f.app.world_mut(), f.request.clone());
    let replies = events::<StartAgentResult>(f.app.world_mut());
    assert_eq!(
        replies[0].result.as_ref().unwrap_err().kind,
        AgentErrorKind::DriverNotLoaded
    );
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn failed_stop_retains_host_and_never_reuses_consumed_end_channel() {
    let mut f = fixture();
    let (entity, request) = start(&mut f).await;
    let instance = make_running(&mut f, entity, &request);
    handle_stop_agent(
        f.app.world_mut(),
        StopAgent {
            id: "first".into(),
            agent: entity,
        },
    );
    let stops = events::<StopDriverLuaVm>(f.app.world_mut());
    handle_stop_driver_lua_vm_result(
        f.app.world_mut(),
        StopDriverLuaVmResult {
            id: stops[0].id.clone(),
            owner: entity,
            instance,
            result: Err(LuaRuntimeError::new(
                LuaRuntimeErrorKind::CleanupFailed,
                "activity cleanup failed",
            )),
        },
    );
    let replies = events::<StopAgentResult>(f.app.world_mut());
    assert_eq!(replies.len(), 1);
    assert!(replies[0].result.is_err());
    let agent = f.app.world().get_component::<Agent>(entity).unwrap();
    assert!(matches!(
        agent.state,
        AgentState::Stopping {
            vm_request_id: None,
            ..
        }
    ));
    assert!(agent.lua.close.is_some());
    handle_stop_agent(
        f.app.world_mut(),
        StopAgent {
            id: "retry".into(),
            agent: entity,
        },
    );
    assert!(events::<StopDriverLuaVm>(f.app.world_mut()).is_empty());
    assert!(f
        .app
        .world()
        .event_reader::<StopAgentResult>()
        .into_iter()
        .next()
        .unwrap()
        .result
        .is_err());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn local_exception_preserves_error_and_rejects_mismatched_identity() {
    let mut f = fixture();
    let (entity, request) = start(&mut f).await;
    let instance = make_running(&mut f, entity, &request);
    let agent = f.app.world().get_component::<Agent>(entity).unwrap();
    let mut event = AgentException {
        exception_id: "local".into(),
        agent: entity,
        resource_id: agent.resourceid.clone(),
        workspace: agent.workspace.clone(),
        instance,
        error: AgentError::new(AgentErrorKind::BlockOperationFailed, "bad block"),
        cleanup_completed: false,
    };
    event.resource_id = id("agent", "wrong");
    handle_agent_exception(f.app.world_mut(), event.clone());
    let rejected = events::<AgentExceptionResult>(f.app.world_mut());
    assert!(rejected[0].result.is_err());
    assert!(!f
        .app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution
        .is_closed());
    event.resource_id = f.request.resource_id.clone();
    handle_agent_exception(f.app.world_mut(), event.clone());
    handle_agent_exception(f.app.world_mut(), event.clone());
    let replies = events::<AgentExceptionResult>(f.app.world_mut());
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].error, Some(event.error));
    assert!(!replies[0].cleanup_completed);
    assert!(f
        .app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution
        .is_closed());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn database_handle_persists_blocks_across_agent_restart() {
    use margatroid_types::{
        block_storage::*,
        block_types::{BlockPath, InnerType},
    };
    let mut f = fixture();
    let (entity, vm) = start(&mut f).await;
    let context = BlockStorageContext {
        agent: f.request.resource_id.clone(),
        instance: DriverLuaVmInstance {
            owner: entity,
            start_request_id: vm.id,
        },
    };
    let execute = |context: BlockStorageContext, operation| BlockStorageRequest {
        context,
        operation,
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    let storage = f
        .app
        .world()
        .agent_block_storage(&f.request.resource_id)
        .unwrap();
    let schema = BlockSchema {
        name: "conversation".into(),
        fields: vec![("messages".into(), InnerType::Message)],
        temporary: false,
    };
    let opened = storage
        .execute(execute(
            context.clone(),
            BlockStorageOperation::Create(schema.clone()),
        ))
        .await
        .unwrap();
    assert!(matches!(
        opened,
        StoredBlockResult::BlockOpened(BlockOpened { created: true, .. })
    ));
    let selector = BlockSelector {
        path: BlockPath {
            block_id: "conversation".into(),
            inner_id: "messages".into(),
        },
        range: BlockRange::All,
    };
    let values =
        StoredBlockInner::Message(vec![margatroid_types::message_types::AgentMessage::User {
            images: Vec::new(),
            content: "persisted".into(),
        }]);
    storage
        .execute(execute(
            context.clone(),
            BlockStorageOperation::Inject {
                source: BlockExpression::Sources(vec![BlockSource::Values(values.clone())]),
                target: selector.clone(),
            },
        ))
        .await
        .unwrap();
    drop(storage);
    f.app.world_mut().despawn(entity);
    let (entity, vm) = start(&mut f).await;
    let context = BlockStorageContext {
        agent: f.request.resource_id.clone(),
        instance: DriverLuaVmInstance {
            owner: entity,
            start_request_id: vm.id,
        },
    };
    let storage = f
        .app
        .world()
        .agent_block_storage(&f.request.resource_id)
        .unwrap();
    assert!(matches!(
        storage
            .execute(execute(
                context.clone(),
                BlockStorageOperation::Create(schema)
            ))
            .await
            .unwrap(),
        StoredBlockResult::BlockOpened(BlockOpened { created: false, .. })
    ));
    assert_eq!(
        storage
            .execute(execute(
                context,
                BlockStorageOperation::Get {
                    expressions: vec![BlockExpression::Sources(vec![BlockSource::Selector(
                        selector
                    )])]
                }
            ))
            .await
            .unwrap(),
        StoredBlockResult::Arrays(vec![values])
    );
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn preparing_duplicate_is_guarded_and_workspace_close_never_spawns_agent() {
    let mut f = fixture();
    handle_start_agent(f.app.world_mut(), f.request.clone());
    handle_start_agent(f.app.world_mut(), f.request.clone());
    assert!(events::<StartAgentResult>(f.app.world_mut()).is_empty());
    let mut duplicate = f.request.clone();
    duplicate.id = "second".into();
    handle_start_agent(f.app.world_mut(), duplicate);
    assert_eq!(
        events::<StartAgentResult>(f.app.world_mut())[0]
            .result
            .as_ref()
            .unwrap_err()
            .kind,
        AgentErrorKind::DuplicateAgent
    );
    f.app
        .world()
        .get_component::<Workspace>(f.request.workspace)
        .unwrap()
        .close_admission();
    let output = database_result(&mut f).await;
    handle_database_setup_result(f.app.world_mut(), output.clone());
    handle_database_setup_result(f.app.world_mut(), output);
    let results = events::<StartAgentResult>(f.app.world_mut());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, f.request.id);
    assert_eq!(
        results[0].result.as_ref().unwrap_err().kind,
        AgentErrorKind::WorkspaceNotAlive
    );
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
    assert!(events::<StartDriverLuaVm>(f.app.world_mut()).is_empty());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn database_preparation_retains_driver_snapshot_and_rejects_stale_association() {
    let mut f = fixture();
    handle_start_agent(f.app.world_mut(), f.request.clone());
    f.app
        .world_mut()
        .get_component_mut::<ResourceSet<AgentDriver>>(f.request.workspace)
        .unwrap()
        .remove(&f.request.driver);
    let output = database_result(&mut f).await;
    let mut stale = output.clone().unwrap();
    stale.operation_id = "stale-database".into();
    handle_database_setup_result(f.app.world_mut(), Ok(stale));
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
    handle_database_setup_result(f.app.world_mut(), output.clone());
    handle_database_setup_result(f.app.world_mut(), output);
    let requests = events::<StartDriverLuaVm>(f.app.world_mut());
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].program.source.as_ref(), "ready()");
    let agent = f
        .app
        .world()
        .get_component::<Agent>(requests[0].owner)
        .unwrap();
    assert_eq!(agent.info.model.model, "model-one");
    assert_eq!(agent.info.driver_dependencies.len(), 1);
    assert!(agent.database.history().list().unwrap().is_empty());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn database_open_failure_has_no_host_or_vm() {
    let mut f = fixture();
    f.request.definition.history_path = f._temp.path().to_path_buf();
    handle_start_agent(f.app.world_mut(), f.request.clone());
    let output = database_result(&mut f).await;
    handle_database_setup_result(f.app.world_mut(), output);
    let results = events::<StartAgentResult>(f.app.world_mut());
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].result.as_ref().unwrap_err().kind,
        AgentErrorKind::DatabaseSetupFailed
    );
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
    assert!(events::<StartDriverLuaVm>(f.app.world_mut()).is_empty());
    f.close.close().await.unwrap();
}
