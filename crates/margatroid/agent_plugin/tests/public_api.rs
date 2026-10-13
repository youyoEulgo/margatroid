use agent_plugin::*;
use app_runtime_plugin::RuntimePlugin;
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
fn fixture() -> Fixture {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ResourcePlugin);
    let world = app.world_mut();
    world.insert_resource(margatroid_types::mcl_environment::MclEnvironmentFactory(
        Arc::new(|| margatroid_types::lua_environment::LuaEnvironment::default()),
    ));
    world.register_library::<Workspace>("workspace").unwrap();
    world
        .register_resources::<ResourceSet<AgentImage>>("image")
        .unwrap();
    world
        .register_resources::<ResourceSet<ToolResource>>("tool")
        .unwrap();
    world
        .register_resources::<ResourceSet<SkillResource>>("skill")
        .unwrap();
    world
        .register_resources::<ResourceSet<HookResource>>("hook")
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
        image: id("image", "base"),
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
        manager: "one".into(),
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
                    pending_images: BTreeMap::new(),
                    image_members: BTreeMap::new(),
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
    let mut images = ResourceSet::new();
    images.insert(
        request.image.clone(),
        AgentImage {
            base_driver: Arc::from("ready()"),
            origin: Arc::new(temp.path().join("driver.lua")),
            dependencies: Arc::from([id("tool", "not-preloaded")]),
            model: AgentImageModelConfig {
                model: "model-one".into(),
                temperature: Some(0.2),
                max_output_tokens: None,
                top_p: None,
                stop: vec![],
            },
        },
    );
    world.insert_resources(workspace, images).unwrap();
    app.add_plugin(AgentPlugin::default());
    Fixture {
        app,
        request,
        close,
        _temp: temp,
    }
}
fn start(f: &mut Fixture) -> (Entity, StartDriverLuaVm) {
    handle_start_agent(f.app.world_mut(), f.request.clone());
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
async fn start_waits_for_ready_and_keeps_resources_empty() {
    let mut f = fixture();
    let (entity, request) = start(&mut f);
    let world = f.app.world();
    let agent = world.get_component::<Agent>(entity).unwrap();
    assert!(matches!(agent.state, AgentState::Creating { .. }));
    assert_eq!(agent.info.model.model, "model-one");
    assert_eq!(agent.info.image_dependencies.len(), 1);
    assert!(agent.history.list().unwrap().is_empty());
    assert!(world
        .get_component::<ResourceSet<ToolResource>>(entity)
        .unwrap()
        .is_empty());
    assert!(world
        .get_component::<ResourceSet<SkillResource>>(entity)
        .unwrap()
        .is_empty());
    assert!(world
        .get_component::<ResourceSet<HookResource>>(entity)
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
        let (entity, request) = start(&mut f);
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
    let (entity, request) = start(&mut f);
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
async fn driver_exception_maps_once_without_inventing_original_error() {
    let mut f = fixture();
    let (entity, request) = start(&mut f);
    let instance = make_running(&mut f, entity, &request);
    let event = DriverLuaVmExceptionResult {
        exception_id: "exception".into(),
        instance,
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
async fn image_lookup_never_uses_another_entity() {
    let mut f = fixture();
    f.app
        .world_mut()
        .get_component_mut::<ResourceSet<AgentImage>>(f.request.workspace)
        .unwrap()
        .remove(&f.request.image);
    handle_start_agent(f.app.world_mut(), f.request.clone());
    let replies = events::<StartAgentResult>(f.app.world_mut());
    assert_eq!(
        replies[0].result.as_ref().unwrap_err().kind,
        AgentErrorKind::ImageNotLoaded
    );
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
    f.close.close().await.unwrap();
}

#[tokio::test]
async fn failed_stop_retains_host_and_never_reuses_consumed_end_channel() {
    let mut f = fixture();
    let (entity, request) = start(&mut f);
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
    let (entity, request) = start(&mut f);
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

#[test]
fn ordinary_block_namespace_type_and_range_invariants_are_preserved() {
    use margatroid_types::block_types::{AgentBlock, BlockInner, BlockPath, InnerType};
    let mut block = AgentBlock::default();
    block
        .create(
            "conversation".into(),
            vec![("messages".into(), InnerType::Message)],
        )
        .unwrap();
    assert!(block.create("conversation".into(), vec![]).is_err());
    let path = BlockPath {
        block_id: "conversation".into(),
        inner_id: "messages".into(),
    };
    assert!(block
        .replace(&path, 0, 0, BlockInner::Tool(vec![]))
        .is_err());
    assert!(block.get(&path).unwrap().is_empty());
    assert!(block
        .replace(&path, 0, 1, BlockInner::Message(vec![]))
        .is_err());
    assert!(block.get(&path).unwrap().is_empty());
}
