use crate::*;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::{Component, Entity, Resource, World};
use margatroid_types::{
    execution_components::AgentExecutionScope,
    lua_environment::{
        LuaBindingValue, LuaEnvironment, LuaGlobalBinding, LuaProgram, LuaStandardLibraries,
        LuaValue,
    },
    lua_runtime_components::{AgentLua, DriverLuaVmInstance, LuaRuntimeErrorKind},
    lua_runtime_events::*,
    mcl_events::AgentMclQueue,
    resource_components::*,
    workspace_components::{Workspace, WorldWorkspaceExt},
};
use resource_id_plugin::{LibraryComponent, ResourceSet, WorldResourceExt};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct HandledExceptions(HashSet<String>);
impl Component for HandledExceptions {}
fn next_id(kind: &str) -> String {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let session = SESSION.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must follow Unix epoch")
            .as_nanos();
        format!("{}:{nanos}", std::process::id())
    });
    format!(
        "agent:{kind}:{session}:{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn error(kind: AgentErrorKind, message: impl Into<String>) -> AgentError {
    AgentError::new(kind, message)
}
fn close_admission(agent: &mut Agent) {
    agent.execution.request_close();
    agent.mcl.accepting = false;
    agent.mcl.drained.close();
    if let Some(pending) = &agent.mcl.pending {
        pending.cancel.cancel();
    }
}
fn environment(agent: &Agent, entity: Entity) -> LuaEnvironment {
    let mut info = BTreeMap::new();
    info.insert("entity".into(), LuaValue::String(format!("{entity:?}")));
    info.insert(
        "resource_id".into(),
        LuaValue::String(agent.resourceid.to_string()),
    );
    info.insert(
        "workspace".into(),
        LuaValue::String(agent.workspace.to_string()),
    );
    info.insert(
        "image".into(),
        LuaValue::String(agent.info.image.to_string()),
    );
    let mut model = BTreeMap::new();
    model.insert(
        "model".into(),
        LuaValue::String(agent.info.model.model.clone()),
    );
    model.insert(
        "temperature".into(),
        agent
            .info
            .model
            .temperature
            .map(|value| LuaValue::Number(value as f64))
            .unwrap_or(LuaValue::Nil),
    );
    model.insert(
        "max_output_tokens".into(),
        agent
            .info
            .model
            .max_output_tokens
            .map(|value| LuaValue::Integer(value as i64))
            .unwrap_or(LuaValue::Nil),
    );
    model.insert(
        "top_p".into(),
        agent
            .info
            .model
            .top_p
            .map(|value| LuaValue::Number(value as f64))
            .unwrap_or(LuaValue::Nil),
    );
    model.insert(
        "stop".into(),
        LuaValue::Array(
            agent
                .info
                .model
                .stop
                .iter()
                .cloned()
                .map(LuaValue::String)
                .collect(),
        ),
    );
    info.insert("model".into(), LuaValue::Object(model));
    info.insert(
        "image_dependencies".into(),
        LuaValue::Array(
            agent
                .info
                .image_dependencies
                .iter()
                .map(|id| LuaValue::String(id.to_string()))
                .collect(),
        ),
    );
    info.insert(
        "project_root".into(),
        LuaValue::String(agent.info.project_root.to_string_lossy().into()),
    );
    info.insert(
        "home_root".into(),
        LuaValue::String(agent.info.home_root.to_string_lossy().into()),
    );
    LuaEnvironment {
        globals: vec![LuaGlobalBinding {
            name: "agent_info".into(),
            binding: LuaBindingValue::Value(LuaValue::Object(info)),
        }],
        modules: vec![],
    }
}

pub fn handle_start_agent(world: &mut World, request: StartAgent) {
    if world.get_resource::<PendingDatabases>().is_some_and(|pending| pending.0.values().any(|value| value.request.id == request.id))
        || world.query_with::<Agent>().result().into_iter().any(|entity| {
            matches!(&world.get_component::<Agent>(entity).unwrap().state, AgentState::Creating { request_id, .. } if request_id == &request.id)
        }) {
        return;
    }
    let prepared = (|| {
        if request.id.is_empty()
            || request.resource_id.resource_type() != "agent"
            || request.image.resource_type() != "image"
        {
            return Err(error(
                AgentErrorKind::InvalidRequest,
                "invalid request ID or resource type",
            ));
        }
        if request.definition.project_root.as_os_str().is_empty()
            || request.definition.home_root.as_os_str().is_empty()
            || request.definition.history_path.as_os_str().is_empty()
        {
            return Err(error(
                AgentErrorKind::InvalidRequest,
                "member paths must be explicit and nonempty",
            ));
        }
        if world
            .get_resource::<PendingDatabases>()
            .is_some_and(|pending| {
                pending
                    .0
                    .values()
                    .any(|value| value.request.resource_id == request.resource_id)
            })
        {
            return Err(error(
                AgentErrorKind::DuplicateAgent,
                "agent address already preparing",
            ));
        }
        let existing = world
            .find_resource_entities(&request.resource_id)
            .map_err(|e| error(AgentErrorKind::ResourceResolutionFailed, e.to_string()))?;
        if !existing.is_empty() {
            return Err(error(
                AgentErrorKind::DuplicateAgent,
                "agent address already exists",
            ));
        }
        let workspace = world
            .get_component::<Workspace>(request.workspace)
            .ok_or_else(|| error(AgentErrorKind::WorkspaceNotAlive, "workspace missing"))?;
        let workspace_id = workspace.resource_id().clone();
        if world
            .find_resource_entities(&workspace_id)
            .map_err(|e| error(AgentErrorKind::ResourceResolutionFailed, e.to_string()))?
            != vec![request.workspace]
        {
            return Err(error(
                AgentErrorKind::WorkspaceNotAlive,
                "workspace identity is not unique",
            ));
        }
        let entry = world
            .workspace_start_entry(request.workspace, &request.id)
            .map_err(|e| error(AgentErrorKind::WorkspaceNotAlive, e.to_string()))?;
        let image = world
            .get_component::<ResourceSet<AgentImage>>(request.workspace)
            .and_then(|set| set.get(&request.image))
            .cloned()
            .ok_or_else(|| {
                error(
                    AgentErrorKind::ImageNotLoaded,
                    "image not loaded on requested workspace",
                )
            })?;
        let mcl_environment = world
            .get_resource::<margatroid_types::mcl_environment::MclEnvironmentFactory>()
            .ok_or_else(|| {
                error(
                    AgentErrorKind::ResourceSetupFailed,
                    "MCL environment factory missing",
                )
            })?
            .build();
        if entry.is_closed() {
            return Err(error(
                AgentErrorKind::WorkspaceNotAlive,
                "workspace admission closed",
            ));
        }
        Ok((workspace_id, image, mcl_environment))
    })();
    let (workspace, image, mcl_environment) = match prepared {
        Ok(value) => value,
        Err(error) => {
            world.send_event(StartAgentResult {
                id: request.id,
                result: Err(error),
            });
            return;
        }
    };
    let operation_id = next_id("database");
    let path = request.definition.history_path.clone();
    let target = request.workspace;
    if !world.contains_resource::<PendingDatabases>() {
        world.insert_resource(PendingDatabases::default());
    }
    world
        .get_resource_mut::<PendingDatabases>()
        .unwrap()
        .0
        .insert(
            operation_id.clone(),
            PendingDatabase {
                request,
                workspace,
                image,
                mcl_environment,
            },
        );
    let events = world.event_sender();
    world.spawn_async_service(async move {
        let result =
            tokio::task::spawn_blocking(move || memory_plugin::SqliteAgentDatabase::open(path))
                .await
                .map_err(|e| error(AgentErrorKind::DatabaseSetupFailed, e.to_string()))
                .and_then(|result| {
                    result.map_err(|e| error(AgentErrorKind::DatabaseSetupFailed, e.to_string()))
                })
                .map(|store| DatabaseSetupOutput {
                    operation_id: operation_id.clone(),
                    workspace: target,
                    database: margatroid_types::block_storage::AgentDatabaseHandle::new(store),
                })
                .map_err(|error| DatabaseSetupError {
                    operation_id,
                    workspace: target,
                    error,
                });
        events.send_event(result);
    });
}

#[derive(Default)]
struct PendingDatabases(BTreeMap<String, PendingDatabase>);
impl Resource for PendingDatabases {}
struct PendingDatabase {
    request: StartAgent,
    workspace: resource_id_plugin::ResourceID,
    image: AgentImage,
    mcl_environment: LuaEnvironment,
}

pub fn handle_database_setup_result(
    world: &mut World,
    result: Result<DatabaseSetupOutput, DatabaseSetupError>,
) {
    let (operation_id, target) = match &result {
        Ok(output) => (&output.operation_id, output.workspace),
        Err(error) => (&error.operation_id, error.workspace),
    };
    let Some(pending) = world
        .get_resource_mut::<PendingDatabases>()
        .and_then(|pending| {
            if pending
                .0
                .get(operation_id)
                .is_some_and(|value| value.request.workspace == target)
            {
                pending.0.remove(operation_id)
            } else {
                None
            }
        })
    else {
        return;
    };
    let PendingDatabase {
        request,
        workspace,
        image,
        mut mcl_environment,
    } = pending;
    let attached = (|| {
        let database = result.map_err(|error| error.error)?.database;
        let component = world.get_component::<Workspace>(target).ok_or_else(|| {
            error(
                AgentErrorKind::WorkspaceNotAlive,
                "workspace missing after database setup",
            )
        })?;
        if component.resource_id() != &workspace
            || world
                .find_resource_entities(&workspace)
                .map_err(|e| error(AgentErrorKind::ResourceResolutionFailed, e.to_string()))?
                != vec![target]
        {
            return Err(error(
                AgentErrorKind::WorkspaceNotAlive,
                "workspace identity changed",
            ));
        }
        let entry = world
            .workspace_start_entry(target, &request.id)
            .map_err(|e| error(AgentErrorKind::WorkspaceNotAlive, e.to_string()))?;
        if entry.is_closed() {
            return Err(error(
                AgentErrorKind::WorkspaceNotAlive,
                "workspace admission closed",
            ));
        }
        if !world
            .find_resource_entities(&request.resource_id)
            .map_err(|e| error(AgentErrorKind::ResourceResolutionFailed, e.to_string()))?
            .is_empty()
        {
            return Err(error(
                AgentErrorKind::DuplicateAgent,
                "agent address occupied during database setup",
            ));
        }
        Ok((database, entry))
    })();
    let (database, execution_entry) = match attached {
        Ok(value) => value,
        Err(error) => {
            world.send_event(StartAgentResult {
                id: request.id,
                result: Err(error),
            });
            return;
        }
    };
    let history = database.history();
    let entity = world.spawn();
    let vm_request_id = next_id("start");
    let agent = Agent {
        resourceid: request.resource_id,
        workspace,
        info: AgentInfo {
            model: image.model.clone(),
            project_root: request.definition.project_root,
            home_root: request.definition.home_root,
            image: request.image,
            image_dependencies: image.dependencies.clone(),
        },
        sandbox: None,
        import_bindings: Default::default(),
        lua: AgentLua::default(),
        state: AgentState::Creating {
            request_id: request.id.clone(),
            vm_request_id: vm_request_id.clone(),
        },
        history,
        database,
        mcl: AgentMclQueue::default(),
        execution: AgentExecutionScope::new(),
        turn: None,
    };
    let mut environment = environment(&agent, entity);
    environment.globals.append(&mut mcl_environment.globals);
    environment.modules.append(&mut mcl_environment.modules);
    let mounted = (|| {
        world.insert_library(entity, agent)?;
        world.insert_resources(entity, ResourceSet::<ToolResource>::new())?;
        world.insert_resources(entity, ResourceSet::<SkillResource>::new())?;
        world.insert_resources(entity, ResourceSet::<HookResource>::new())?;
        world.insert_resources(entity, ResourceSet::<PromptResource>::new())?;
        world.insert_resources(entity, ResourceSet::<SandboxResource>::new())?;
        Ok::<_, resource_id_plugin::ResourceError>(())
    })();
    if let Err(e) = mounted {
        world.despawn(entity);
        world.send_event(StartAgentResult {
            id: request.id,
            result: Err(error(AgentErrorKind::ResourceSetupFailed, e.to_string())),
        });
        return;
    }
    world.insert_component(entity, HandledExceptions::default());
    world.send_event(StartDriverLuaVm {
        id: vm_request_id,
        owner: entity,
        program: LuaProgram {
            source: image.base_driver,
            origin: image.origin,
            libraries: LuaStandardLibraries::Safe,
        },
        environment,
        execution_entry,
    });
}

pub fn handle_start_driver_lua_vm_result(world: &mut World, event: StartDriverLuaVmResult) {
    let Some(agent) = world.get_component_mut::<Agent>(event.owner) else {
        return;
    };
    let AgentState::Creating {
        request_id,
        vm_request_id,
    } = &agent.state
    else {
        return;
    };
    if vm_request_id != &event.id {
        return;
    }
    let request_id = request_id.clone();
    let expected = DriverLuaVmInstance {
        owner: event.owner,
        start_request_id: event.id.clone(),
    };
    let result = match event.result {
        Ok(())
            if agent.lua.instance.as_ref() == Some(&expected)
                && agent
                    .lua
                    .close
                    .as_ref()
                    .is_some_and(|close| close.instance() == expected) =>
        {
            agent.lua.ready = None;
            agent.state = AgentState::Running;
            Ok(event.owner)
        }
        Ok(()) => {
            close_admission(agent);
            agent.state = AgentState::Stopping {
                request_ids: vec![],
                vm_request_id: None,
            };
            Err(error(
                AgentErrorKind::DriverStartFailed,
                "ready result lacks matching instance or close handle",
            ))
        }
        Err(e) => {
            if e.kind == LuaRuntimeErrorKind::CleanupFailed {
                close_admission(agent);
                agent.state = AgentState::Stopping {
                    request_ids: vec![],
                    vm_request_id: None,
                };
            } else {
                world.despawn(event.owner);
            }
            Err(error(AgentErrorKind::DriverStartFailed, e.to_string()))
        }
    };
    world.send_event(StartAgentResult {
        id: request_id,
        result,
    });
}

pub fn handle_stop_agent(world: &mut World, request: StopAgent) {
    let Some(agent) = world.get_component_mut::<Agent>(request.agent) else {
        tracing::warn!(agent = ?request.agent, "stop target missing");
        world.send_event(StopAgentResult {
            id: request.id,
            result: Err(error(AgentErrorKind::AgentNotAlive, "agent missing")),
        });
        return;
    };
    let rejection = if request.id.is_empty() {
        Some(error(AgentErrorKind::InvalidRequest, "empty request ID"))
    } else {
        match &mut agent.state {
            AgentState::Creating { .. } => {
                Some(error(AgentErrorKind::AgentBusy, "agent still creating"))
            }
            AgentState::Stopping {
                request_ids,
                vm_request_id: Some(_),
            } => {
                if !request_ids.contains(&request.id) {
                    request_ids.push(request.id);
                }
                return;
            }
            AgentState::Stopping { .. } => Some(error(
                AgentErrorKind::DriverStopFailed,
                "previous stop failed; no new completion proof",
            )),
            AgentState::Running => None,
        }
    };
    if let Some(e) = rejection {
        world.send_event(StopAgentResult {
            id: request.id,
            result: Err(e),
        });
        return;
    }
    close_admission(agent);
    let Some(instance) = agent.lua.instance.clone() else {
        agent.state = AgentState::Stopping {
            request_ids: vec![],
            vm_request_id: None,
        };
        world.send_event(StopAgentResult {
            id: request.id,
            result: Err(error(
                AgentErrorKind::DriverStopFailed,
                "missing driver instance",
            )),
        });
        return;
    };
    let id = next_id("stop");
    agent.state = AgentState::Stopping {
        request_ids: vec![request.id],
        vm_request_id: Some(id.clone()),
    };
    world.send_event(StopDriverLuaVm {
        id,
        owner: request.agent,
        instance,
    });
}

pub fn handle_stop_driver_lua_vm_result(world: &mut World, event: StopDriverLuaVmResult) {
    let Some(agent) = world.get_component_mut::<Agent>(event.owner) else {
        return;
    };
    if agent.lua.instance.as_ref() != Some(&event.instance) || event.instance.owner != event.owner {
        return;
    }
    let AgentState::Stopping {
        request_ids,
        vm_request_id,
    } = &mut agent.state
    else {
        return;
    };
    if vm_request_id.as_ref() != Some(&event.id) {
        return;
    }
    let ids = std::mem::take(request_ids);
    *vm_request_id = None;
    let result = event
        .result
        .map(|()| event.owner)
        .map_err(|e| error(AgentErrorKind::DriverStopFailed, e.to_string()));
    if result.is_ok() {
        world.despawn(event.owner);
    }
    for id in ids {
        world.send_event(StopAgentResult {
            id,
            result: result.clone(),
        });
    }
}

fn first_exception(world: &mut World, entity: Entity, id: &str) -> bool {
    if !world.contains_component::<HandledExceptions>(entity) {
        world.insert_component(entity, HandledExceptions::default());
    }
    world
        .get_component_mut::<HandledExceptions>(entity)
        .is_some_and(|handled| handled.0.insert(id.into()))
}
pub fn handle_driver_lua_vm_exception_result(world: &mut World, event: DriverLuaVmExceptionResult) {
    let entity = event.instance.owner;
    let Some(agent) = world.get_component::<Agent>(entity) else {
        return;
    };
    if agent.lua.instance.as_ref() != Some(&event.instance) || event.exception_id.is_empty() {
        return;
    }
    let resource_id = agent.resourceid.clone();
    let workspace = agent.workspace.clone();
    if !first_exception(world, entity, &event.exception_id) {
        return;
    }
    close_admission(world.get_component_mut::<Agent>(entity).unwrap());
    world.send_event(AgentExceptionResult {
        exception_id: event.exception_id,
        agent: entity,
        resource_id,
        workspace,
        instance: event.instance,
        error: None,
        result: event
            .result
            .map_err(|e| error(AgentErrorKind::DriverStopFailed, e.to_string())),
        execution_ended: event.execution_ended,
        cleanup_completed: event.cleanup_completed,
    });
}
pub fn handle_agent_exception(world: &mut World, event: AgentException) {
    let valid = world
        .get_component::<Agent>(event.agent)
        .is_some_and(|agent| {
            agent.resourceid == event.resource_id
                && agent.workspace == event.workspace
                && agent.lua.instance.as_ref() == Some(&event.instance)
                && event.instance.owner == event.agent
                && !event.exception_id.is_empty()
        });
    if valid && !first_exception(world, event.agent, &event.exception_id) {
        return;
    }
    let result = if valid {
        close_admission(world.get_component_mut::<Agent>(event.agent).unwrap());
        Ok(())
    } else {
        Err(error(
            AgentErrorKind::InvalidRequest,
            "exception instance association mismatch",
        ))
    };
    world.send_event(AgentExceptionResult {
        exception_id: event.exception_id,
        agent: event.agent,
        resource_id: event.resource_id,
        workspace: event.workspace,
        instance: event.instance,
        error: Some(event.error),
        result,
        execution_ended: valid && event.cleanup_completed,
        cleanup_completed: valid && event.cleanup_completed,
    });
}
