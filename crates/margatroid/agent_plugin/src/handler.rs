use std::collections::BTreeMap;

use core_plugin::{Entity, World};
use lua_runtime_plugin::{
    LuaEnvironmentContext, LuaRuntimeHandle, LuaRuntimeReply, LuaRuntimeRequest,
    LuaRuntimeTaskFinished, LuaScheduler, LuaValue, LuaVmOwner, LuaVmStarted,
};
use resource_id_plugin::{ResourceId, ResourceRef, WorldResourceIdExt};

use crate::{
    failure, Agent, AgentControl, AgentControlKind, AgentCreateRequest, AgentCreationState,
    AgentFailureKind, AgentInferenceState, AgentInfo, AgentInitializationCompleted,
    AgentLifecycleState, AgentLuaState, AgentMcl, AgentResourceMap, AgentToolState, AgentTurnState,
    TokenUsageState,
};

fn agent_label(world: &World, entity: Entity) -> String {
    world
        .get_component::<ResourceId>(entity)
        .map(|id| id.as_ref().to_string())
        .unwrap_or_else(|| format!("Entity({entity:?})"))
}

pub fn handle_agent_create(world: &mut World, request: AgentCreateRequest) {
    let invalid_reason = if request.id.is_empty() {
        Some("request id is empty")
    } else if request.agent_id.resource_type() != "agent" {
        Some("agent id is not an agent resource")
    } else if !world.is_alive(request.workspace_id) {
        Some("workspace is not alive")
    } else if world
        .query_with::<ResourceId>()
        .result()
        .into_iter()
        .any(|entity| {
            world
                .get_component::<ResourceId>(entity)
                .map(ResourceId::as_ref)
                == Some(&request.agent_id)
        })
    {
        Some("agent resource id already exists")
    } else if world
        .query_with::<Agent>()
        .result()
        .into_iter()
        .any(|entity| {
            world
                .get_component::<Agent>(entity)
                .is_some_and(|agent| agent.creation.request_id == request.id)
        })
    {
        Some("creation request id already exists")
    } else {
        None
    };
    if let Some(reason) = invalid_reason {
        tracing::error!(
            request_id = %request.id,
            agent_id = %request.agent_id,
            reason,
            "agent creation request is invalid"
        );
        request.reply.send(Err(failure(
            AgentFailureKind::InvalidRequest,
            format!("agent creation request is invalid: {reason}"),
        )));
        return;
    }

    let entity = world.spawn();
    let info = AgentInfo {
        image_entity: request.image_entity,
        workspace_id: request.workspace_id,
        model: request.model.clone(),
        project_root: request.project_root.clone(),
        image_root: request.image_root.clone(),
        home_root: request.home_root.clone(),
        image_dependencies: request.image_dependencies.clone(),
    };
    let agent = Agent {
        info: info.clone(),
        creation: AgentCreationState {
            request_id: request.id.clone(),
            reply: request.reply.clone(),
            initialization: Default::default(),
        },
        mcl: AgentMcl::default(),
        resources: AgentResourceMap::default(),
        memory: request.memory,
        inference: AgentInferenceState {
            model: request.model,
            pending: Default::default(),
        },
        tools: AgentToolState::default(),
        lua: AgentLuaState {
            request_id: Some(request.id.clone()),
            vm_id: None,
        },
        lifecycle: AgentLifecycleState::Creating,
        turn: AgentTurnState::default(),
        token_usage: TokenUsageState::from(&request.token_usage),
        last_error: None,
    };

    if world
        .attach_resource_id(entity, request.agent_id.clone())
        .is_err()
        || !world.insert_component(entity, agent)
    {
        tracing::error!(request_id = %request.id, "agent components could not be attached");
        world.despawn(entity);
        request.reply.send(Err(failure(
            AgentFailureKind::InvalidRequest,
            "agent components could not be attached",
        )));
        return;
    }

    let Some(runtime) = world.get_resource::<LuaRuntimeHandle>().cloned() else {
        fail_creation(
            world,
            entity,
            failure(
                AgentFailureKind::LuaRuntime,
                "LuaRuntimePlugin is unavailable",
            ),
        );
        return;
    };

    let context = LuaEnvironmentContext {
        request_id: request.id.clone(),
        owner: LuaVmOwner {
            owner_id: request.agent_id.to_string(),
        },
        values: [(
            "agent_info".to_owned(),
            agent_info_value(&request.agent_id, &info),
        )]
        .into_iter()
        .collect(),
    };
    let (sender, _receiver) = tokio::sync::oneshot::channel();
    let runtime_request = LuaRuntimeRequest {
        request_id: request.id,
        owner: context.owner.clone(),
        program: request.base_lua,
        context,
        providers: vec!["agent_info".to_owned(), "mcl".to_owned()],
        scheduler: LuaScheduler::LongRunning,
        deadline: None,
        reply: LuaRuntimeReply::new(sender),
    };
    if let Err(error) = runtime.register_long_running(runtime_request) {
        fail_creation(
            world,
            entity,
            failure(AgentFailureKind::LuaRuntime, error.to_string()),
        );
    }
}

pub fn handle_agent_control(world: &mut World, event: AgentControl) {
    let result = if world.get_component::<ResourceId>(event.agent).is_none()
        || world.get_component::<Agent>(event.agent).is_none()
    {
        Err(failure(
            AgentFailureKind::AgentMissing,
            "target entity is not an addressable agent",
        ))
    } else {
        match event.control {
            AgentControlKind::Stop => control_stop(world, event.agent),
            AgentControlKind::AbortTurn => world
                .get_component_mut::<Agent>(event.agent)
                .ok_or_else(|| failure(AgentFailureKind::AgentMissing, "agent is missing"))
                .map(|agent| {
                    agent.turn.abort();
                }),
        }
    };

    if let Err(error) = &result {
        if let Some(agent) = world.get_component_mut::<Agent>(event.agent) {
            agent.last_error = Some(error.clone());
        }
    }
    event.reply.send(result);
}

pub fn handle_lua_vm_started(world: &mut World, event: LuaVmStarted) {
    let matches = world
        .query_with::<Agent>()
        .result()
        .into_iter()
        .filter(|entity| {
            world
                .get_component::<Agent>(*entity)
                .is_some_and(|agent| agent.lua.request_id.as_deref() == Some(&event.request_id))
                && world
                    .get_component::<ResourceId>(*entity)
                    .is_some_and(|id| id.as_ref().to_string() == event.owner.owner_id)
        })
        .collect::<Vec<_>>();
    let [entity] = matches.as_slice() else {
        return;
    };
    let Some(agent) = world.get_component_mut::<Agent>(*entity) else {
        return;
    };
    if agent.lifecycle != AgentLifecycleState::Creating {
        return;
    }
    agent.lua.request_id = None;
    agent.lua.vm_id = Some(event.vm_id);
}

pub fn handle_agent_initialization_completed(
    world: &mut World,
    event: AgentInitializationCompleted,
) {
    let label = agent_label(world, event.agent);
    let Some(agent) = world.get_component_mut::<Agent>(event.agent) else {
        return;
    };
    if agent.creation.initialization.complete {
        return;
    }
    if agent.lifecycle != AgentLifecycleState::Creating
        || agent.lua.vm_id != Some(event.vm_id)
        || agent.creation.initialization.failed.is_some()
    {
        tracing::warn!(agent = %label, "agent initialization completion does not match the active VM");
        agent.last_error = Some(failure(
            AgentFailureKind::InvalidRequest,
            "agent initialization completion does not match the active VM",
        ));
        return;
    }
    agent.creation.initialization.complete = true;
    agent.lifecycle = AgentLifecycleState::Running;
    tracing::info!(agent = %label, "agent initialization completed");
    agent.creation.reply.send(Ok(event.agent));
}

pub fn handle_lua_vm_finished(world: &mut World, event: LuaRuntimeTaskFinished) {
    let target = world
        .query_with::<Agent>()
        .result()
        .into_iter()
        .find(|entity| {
            world.get_component::<Agent>(*entity).is_some_and(|agent| {
                agent.lua.request_id.as_deref() == Some(&event.request_id)
                    || event
                        .vm_id
                        .is_some_and(|vm_id| agent.lua.vm_id == Some(vm_id))
            })
        });
    let Some(entity) = target else {
        return;
    };
    let label = agent_label(world, entity);
    let Some(agent) = world.get_component_mut::<Agent>(entity) else {
        return;
    };
    let was_creating = !agent.creation.initialization.complete;
    agent.lua.request_id = None;
    agent.lua.vm_id = None;
    if let Some(runtime_error) = event.error {
        tracing::error!(agent = %label, error = %runtime_error, "agent Lua runtime finished with error");
        let error = failure(AgentFailureKind::LuaRuntime, runtime_error.to_string());
        agent.lifecycle = AgentLifecycleState::Failed;
        agent.last_error = Some(error.clone());
        agent
            .creation
            .initialization
            .failed
            .get_or_insert(error.clone());
        if was_creating {
            agent.creation.reply.send(Err(error));
        }
    } else {
        agent.lifecycle = AgentLifecycleState::Stopped;
        if was_creating {
            agent.creation.reply.send(Err(failure(
                AgentFailureKind::Stopped,
                "agent VM stopped before initialization completed",
            )));
        }
    }
}

pub fn control_stop(world: &mut World, entity: Entity) -> Result<(), margatroid_types::AgentError> {
    let agent = world
        .get_component_mut::<Agent>(entity)
        .ok_or_else(|| failure(AgentFailureKind::AgentMissing, "agent is missing"))?;
    if agent.lua.vm_id.is_none() && agent.lua.request_id.is_none() {
        if agent.lifecycle != AgentLifecycleState::Failed {
            agent.lifecycle = AgentLifecycleState::Stopped;
        }
        return Ok(());
    }
    let id = world
        .get_component::<ResourceId>(entity)
        .ok_or_else(|| failure(AgentFailureKind::AgentMissing, "agent identity is missing"))?
        .as_ref()
        .to_string();
    let runtime = world
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .ok_or_else(|| failure(AgentFailureKind::LuaRuntime, "Lua runtime is missing"))?;
    runtime
        .stop_long_running(&id)
        .map_err(|error| failure(AgentFailureKind::LuaRuntime, error.to_string()))?;
    world
        .get_component_mut::<Agent>(entity)
        .expect("agent disappeared during stop")
        .lifecycle = AgentLifecycleState::Stopping;
    Ok(())
}

fn fail_creation(world: &mut World, entity: Entity, error: margatroid_types::AgentError) {
    tracing::error!(agent = %agent_label(world, entity), error = %error, "agent creation failed");
    if let Some(agent) = world.get_component_mut::<Agent>(entity) {
        agent.lifecycle = AgentLifecycleState::Failed;
        agent.lua.request_id = None;
        agent.lua.vm_id = None;
        agent.creation.initialization.failed = Some(error.clone());
        agent.last_error = Some(error.clone());
        agent.creation.reply.send(Err(error));
    }
}

fn agent_info_value(id: &ResourceRef, info: &AgentInfo) -> LuaValue {
    let mut value = BTreeMap::new();
    value.insert("id".to_owned(), LuaValue::String(id.to_string()));
    value.insert(
        "workspace_id".to_owned(),
        LuaValue::Integer(info.workspace_id.index() as i64),
    );
    value.insert(
        "project_root".to_owned(),
        LuaValue::String(info.project_root.display().to_string()),
    );
    value.insert(
        "image_root".to_owned(),
        LuaValue::String(info.image_root.display().to_string()),
    );
    value.insert(
        "home_root".to_owned(),
        LuaValue::String(info.home_root.display().to_string()),
    );
    value.insert(
        "model".to_owned(),
        LuaValue::Object(
            [
                (
                    "provider".to_owned(),
                    LuaValue::String(info.model.provider.clone()),
                ),
                (
                    "model".to_owned(),
                    LuaValue::String(info.model.model.clone()),
                ),
                (
                    "context_window_tokens".to_owned(),
                    LuaValue::Integer(info.model.context_window_tokens as i64),
                ),
            ]
            .into_iter()
            .collect(),
        ),
    );
    LuaValue::Object(value)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use margatroid_types::{AgentError, LuaVmId, Message, TokenUsage, ToolDefinition};

    use super::*;
    use crate::{AgentMemoryHandle, AgentMemoryStore, AgentMemoryStoreError};

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

        fn history_messages(&self) -> Result<Vec<crate::HistoryMessage>, AgentMemoryStoreError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn vm_start_does_not_complete_creation_before_mcl_start() {
        let mut world = World::new();
        let workspace = world.spawn();
        let image = world.spawn();
        let entity = world.spawn();
        let id: ResourceRef = "agent:test/coder:latest".parse().unwrap();
        let (sender, mut receiver) = tokio::sync::oneshot::channel::<Result<Entity, AgentError>>();
        let model = crate::AgentModelInfo {
            provider: "test".to_owned(),
            model: "test".to_owned(),
            context_window_tokens: 1024,
        };
        world.insert_resource(resource_id_plugin::ResourceIdPluginInstalled);
        world.attach_resource_id(entity, id.clone()).unwrap();
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
                },
                creation: AgentCreationState {
                    request_id: "create-1".to_owned(),
                    reply: crate::AgentCreateReply::new(sender),
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
                    request_id: Some("create-1".to_owned()),
                    vm_id: None,
                },
                lifecycle: AgentLifecycleState::Creating,
                turn: Default::default(),
                token_usage: Default::default(),
                last_error: None,
            },
        );

        let vm_id = LuaVmId(9);
        handle_lua_vm_started(
            &mut world,
            LuaVmStarted {
                request_id: "create-1".to_owned(),
                vm_id,
                owner: LuaVmOwner {
                    owner_id: id.to_string(),
                },
            },
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(
            world.get_component::<Agent>(entity).unwrap().lifecycle,
            AgentLifecycleState::Creating
        );

        handle_agent_initialization_completed(
            &mut world,
            AgentInitializationCompleted {
                agent: entity,
                vm_id,
            },
        );
        assert_eq!(receiver.try_recv().unwrap().unwrap(), entity);
        assert_eq!(
            world.get_component::<Agent>(entity).unwrap().lifecycle,
            AgentLifecycleState::Running
        );
    }
}
