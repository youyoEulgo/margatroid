use crate::agent_events::{AgentError, AgentErrorKind};
use crate::block_storage::{AgentBlockStorageHandle, AgentDatabaseHandle};
use crate::block_types::{BlockInner, InnerType};
use crate::execution_components::{
    AgentExecutionScope, ExecutionActivity, ExecutionActivityKind, SandboxPolicySnapshot,
};
use crate::history_components::{AgentHistoryHandle, HistoryRecord};
use crate::mcl_events::AgentMclQueue;
use crate::message_types::AgentMessage;
use crate::resource_components::{
    AgentImageModelConfig, CallableDefinitionSnapshot, CallableResource, SandboxResource,
    SandboxValue,
};
use async_runtime_plugin::{AsyncRuntimeHandle, WorldAsyncExt};
use core_plugin::{Component, Entity, World};
use resource_id_plugin::{LibraryComponent, ResourceID, ResourceSet, WorldResourceExt};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportedValue {
    Message {
        resource_id: ResourceID,
        messages: Arc<[AgentMessage]>,
    },
    Tool(CallableResource),
    Sandbox(SandboxValue),
}
impl ImportedValue {
    pub fn resource_id(&self) -> &ResourceID {
        match self {
            Self::Message { resource_id, .. } => resource_id,
            Self::Tool(value) => &value.resource_id,
            Self::Sandbox(value) => &value.resource_id,
        }
    }
    pub fn inner_type(&self) -> InnerType {
        match self {
            Self::Message { .. } => InnerType::Message,
            Self::Tool(_) => InnerType::Tool,
            Self::Sandbox(_) => InnerType::Sandbox,
        }
    }
    fn array(&self) -> BlockInner {
        match self {
            Self::Message { messages, .. } => BlockInner::Message(messages.to_vec()),
            Self::Tool(value) => BlockInner::Tool(vec![value.clone()]),
            Self::Sandbox(value) => BlockInner::Sandbox(vec![value.clone()]),
        }
    }
    fn valid_kind(&self) -> bool {
        let kind = self.resource_id().resource_type();
        match self {
            Self::Message { .. } => kind == "prompt",
            Self::Sandbox(_) => kind == "sandbox",
            Self::Tool(value) => matches!(
                (&*value.definition, kind),
                (CallableDefinitionSnapshot::Tool(_), "tool")
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportBindings {
    bindings: BTreeMap<String, ImportedValue>,
}

impl ImportBindings {
    pub fn get(&self, alias: &str) -> Option<&ImportedValue> {
        self.bindings.get(alias)
    }
    pub fn get_resource(
        &self,
        resource_id: &ResourceID,
        kind: InnerType,
    ) -> Option<&ImportedValue> {
        self.bindings
            .values()
            .find(|value| value.resource_id() == resource_id && value.inner_type() == kind)
    }
    pub fn resource_value(
        &self,
        resource_id: &ResourceID,
        kind: InnerType,
    ) -> Option<ImportedValue> {
        self.get_resource(resource_id, kind).cloned()
    }
    pub fn resolve_resource(
        &self,
        resource_id: &ResourceID,
        kind: InnerType,
    ) -> Result<BlockInner, AgentError> {
        self.resource_value(resource_id, kind)
            .map(|value| value.array())
            .ok_or_else(|| {
                AgentError::new(
                    AgentErrorKind::ResourceResolutionFailed,
                    format!("resource {resource_id} is not imported as {kind:?}"),
                )
            })
    }
    pub fn contains_resource(&self, resource_id: &ResourceID, kind: InnerType) -> bool {
        self.bindings
            .values()
            .any(|value| value.resource_id() == resource_id && value.inner_type() == kind)
    }
}

#[derive(Clone, Debug)]
pub struct AgentInfo {
    pub model: AgentImageModelConfig,
    pub project_root: PathBuf,
    pub home_root: PathBuf,
    pub image: ResourceID,
    pub image_dependencies: Arc<[ResourceID]>,
}
#[derive(Clone, Debug)]
pub enum AgentState {
    Creating {
        request_id: String,
        vm_request_id: String,
    },
    Idle,
    Working,
    Stopping {
        request_ids: Vec<String>,
        vm_request_id: Option<String>,
    },
}
/// Last accepted exception, retained on the host for state snapshots.
#[derive(Clone, Debug)]
pub struct AgentFailure(pub AgentError);
impl Component for AgentFailure {}

#[derive(Clone, Debug)]
pub struct AbortedAgentTurn {
    pub instance: crate::lua_runtime_components::DriverLuaVmInstance,
    pub turn_id: String,
}
impl Component for AbortedAgentTurn {}

#[derive(Debug)]
pub struct Agent {
    pub resourceid: ResourceID,
    pub info: AgentInfo,
    pub workspace: ResourceID,
    pub sandbox: Option<SandboxValue>,
    pub lua: crate::lua_runtime_components::AgentLua,
    pub state: AgentState,
    pub history: AgentHistoryHandle,
    pub database: AgentDatabaseHandle,
    pub import_bindings: ImportBindings,
    pub mcl: AgentMclQueue,
    pub execution: AgentExecutionScope,
    pub turn: Option<String>,
}
impl Component for Agent {}
impl LibraryComponent for Agent {
    fn resource_id(&self) -> &ResourceID {
        &self.resourceid
    }
}
impl Agent {
    pub fn new_turn(&mut self) -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        static SESSION: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
        let session = SESSION.get_or_init(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock predates UNIX epoch")
                .as_nanos()
        });
        let id = format!(
            "turn:{}:{}:{session}:{sequence}",
            self.resourceid,
            std::process::id()
        );
        self.turn = Some(id.clone());
        id
    }
}

pub trait WorldAgentExt {
    fn agent_by_id(&self, id: &ResourceID) -> Result<Entity, AgentError>;
    fn agent_switch_sandbox(
        &mut self,
        id: &ResourceID,
        sandbox: Option<SandboxValue>,
    ) -> Result<(), AgentError>;
    fn agent_register_activity(
        &self,
        id: &ResourceID,
        kind: ExecutionActivityKind,
    ) -> Result<ExecutionActivity, AgentError>;
    fn agent_cleanup_tasks(
        &self,
        id: &ResourceID,
    ) -> Result<oneshot::Receiver<Result<(), AgentError>>, AgentError>;
    fn agent_append_history_batch(
        &self,
        id: &ResourceID,
        records: Vec<HistoryRecord>,
    ) -> Result<(), AgentError>;
    fn agent_validate_import(
        &self,
        id: &ResourceID,
        alias: &str,
        kind: InnerType,
        resource_id: &ResourceID,
    ) -> Result<(), AgentError>;
    fn agent_bind_import(
        &mut self,
        id: &ResourceID,
        alias: String,
        value: ImportedValue,
    ) -> Result<(), AgentError>;
    fn agent_get_import(&self, id: &ResourceID, alias: &str) -> Result<BlockInner, AgentError>;
    fn agent_block_storage(&self, id: &ResourceID) -> Result<AgentBlockStorageHandle, AgentError>;
}
impl WorldAgentExt for World {
    fn agent_by_id(&self, id: &ResourceID) -> Result<Entity, AgentError> {
        if id.resource_type() != "agent" {
            return Err(AgentError::new(
                AgentErrorKind::InvalidResourceID,
                "expected agent ID",
            ));
        }
        let entities = self.find_resource_entities(id).map_err(|e| {
            AgentError::new(AgentErrorKind::ResourceResolutionFailed, e.to_string())
        })?;
        match entities.as_slice() {
            [entity] if self.contains_component::<Agent>(*entity) => Ok(*entity),
            [] => Err(AgentError::new(
                AgentErrorKind::AgentNotAlive,
                "agent missing",
            )),
            _ => Err(AgentError::new(
                AgentErrorKind::DuplicateAgent,
                "agent address is not unique",
            )),
        }
    }
    fn agent_switch_sandbox(
        &mut self,
        id: &ResourceID,
        sandbox: Option<SandboxValue>,
    ) -> Result<(), AgentError> {
        let entity = self.agent_by_id(id)?;
        let agent = self.get_component::<Agent>(entity).unwrap();
        if let Some(ref value) = sandbox {
            if value.resource_id.resource_type() != "sandbox"
                || !agent.info.image_dependencies.contains(&value.resource_id)
                || !agent.import_bindings.bindings.values().any(|binding| {
                    matches!(binding,
                    ImportedValue::Sandbox(imported) if imported.resource_id == value.resource_id)
                })
                || self
                    .get_component::<ResourceSet<SandboxResource>>(entity)
                    .and_then(|set| set.get(&value.resource_id))
                    .is_none()
            {
                return Err(AgentError::new(
                    AgentErrorKind::SandboxNotLoaded,
                    "sandbox source is not permitted, imported and loaded on this agent",
                ));
            }
        }
        let changed = agent.sandbox != sandbox;
        let scope = agent.execution.clone();
        scope
            .with_sandbox_configuration(changed, || {
                self.get_component_mut::<Agent>(entity).unwrap().sandbox = sandbox;
                Ok(())
            })
            .map_err(AgentError::from)
    }
    fn agent_register_activity(
        &self,
        id: &ResourceID,
        kind: ExecutionActivityKind,
    ) -> Result<ExecutionActivity, AgentError> {
        let entity = self.agent_by_id(id)?;
        let agent = self.get_component::<Agent>(entity).unwrap();
        agent
            .execution
            .register_for_turn_with_snapshot(kind, agent.turn.clone(), || {
                if kind != ExecutionActivityKind::Sandbox {
                    return Ok(None);
                }
                Ok(agent.sandbox.as_ref().map(|value| SandboxPolicySnapshot {
                    resource_id: value.resource_id.clone(),
                    policy: value.policy.clone(),
                }))
            })
            .map_err(AgentError::from)
    }
    fn agent_cleanup_tasks(
        &self,
        id: &ResourceID,
    ) -> Result<oneshot::Receiver<Result<(), AgentError>>, AgentError> {
        let entity = self.agent_by_id(id)?;
        let scope = self
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .clone();
        if !self.contains_resource::<AsyncRuntimeHandle>() {
            return Err(AgentError::new(
                AgentErrorKind::CleanupFailed,
                "AsyncRuntimePlugin is not installed",
            ));
        }
        let (tx, rx) = oneshot::channel();
        let (start, started) = oneshot::channel();
        let worker_scope = scope.clone();
        let submitted = catch_unwind(AssertUnwindSafe(|| {
            self.spawn_async_service(async move {
                if started.await.is_err() {
                    return;
                }
                let result = match worker_scope.wait_sandbox().await {
                    Ok(()) => worker_scope.resume_sandbox(),
                    Err(error) => Err(error),
                }
                .map_err(AgentError::from);
                let _ = tx.send(result);
            })
        }));
        if submitted.is_err() {
            return Err(AgentError::new(
                AgentErrorKind::CleanupFailed,
                "sandbox cleanup service submission failed",
            ));
        }
        scope.begin_sandbox_cleanup();
        if start.send(()).is_err() {
            return Err(AgentError::new(
                AgentErrorKind::CleanupFailed,
                "sandbox cleanup service exited before start",
            ));
        }
        Ok(rx)
    }
    fn agent_append_history_batch(
        &self,
        id: &ResourceID,
        records: Vec<HistoryRecord>,
    ) -> Result<(), AgentError> {
        let entity = self.agent_by_id(id)?;
        self.get_component::<Agent>(entity)
            .unwrap()
            .history
            .append_batch(records)
            .map_err(|e| AgentError::new(AgentErrorKind::HistoryWriteFailed, e.to_string()))
    }
    fn agent_validate_import(
        &self,
        id: &ResourceID,
        alias: &str,
        kind: InnerType,
        resource_id: &ResourceID,
    ) -> Result<(), AgentError> {
        let entity = self.agent_by_id(id)?;
        let agent = self.get_component::<Agent>(entity).unwrap();
        let valid_kind = matches!(
            (kind, resource_id.resource_type()),
            (InnerType::Message, "prompt")
                | (InnerType::Tool, "tool")
                | (InnerType::Sandbox, "sandbox")
        );
        let mut alias_bytes = alias.bytes();
        let valid_alias = alias_bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
            && alias_bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        if !valid_alias || !valid_kind || !agent.info.image_dependencies.contains(resource_id) {
            return Err(AgentError::new(
                AgentErrorKind::InvalidDefinition,
                "invalid import alias, kind or undeclared source",
            ));
        }
        if agent
            .import_bindings
            .get(alias)
            .is_some_and(|old| old.resource_id() != resource_id || old.inner_type() != kind)
        {
            return Err(AgentError::new(
                AgentErrorKind::AliasConflict,
                "import alias already bound",
            ));
        }
        Ok(())
    }
    fn agent_bind_import(
        &mut self,
        id: &ResourceID,
        alias: String,
        value: ImportedValue,
    ) -> Result<(), AgentError> {
        self.agent_validate_import(id, &alias, value.inner_type(), value.resource_id())?;
        if !value.valid_kind() || matches!(&value, ImportedValue::Tool(tool) if tool.alias != alias)
        {
            return Err(AgentError::new(
                AgentErrorKind::InvalidDefinition,
                "definition kind or alias mismatch",
            ));
        }
        let entity = self.agent_by_id(id)?;
        let agent = self.get_component_mut::<Agent>(entity).unwrap();
        agent
            .import_bindings
            .bindings
            .retain(|_, old| old.resource_id() != value.resource_id());
        agent.import_bindings.bindings.insert(alias, value);
        Ok(())
    }
    fn agent_get_import(&self, id: &ResourceID, alias: &str) -> Result<BlockInner, AgentError> {
        let entity = self.agent_by_id(id)?;
        self.get_component::<Agent>(entity)
            .unwrap()
            .import_bindings
            .bindings
            .get(alias)
            .map(ImportedValue::array)
            .ok_or_else(|| {
                AgentError::new(
                    AgentErrorKind::ResourceResolutionFailed,
                    "import alias missing",
                )
            })
    }
    fn agent_block_storage(&self, id: &ResourceID) -> Result<AgentBlockStorageHandle, AgentError> {
        let entity = self.agent_by_id(id)?;
        Ok(self
            .get_component::<Agent>(entity)
            .unwrap()
            .database
            .block_storage())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_types::{BlockInner, InnerType};
    use crate::execution_components::ExecutionAdmission;
    use crate::execution_components::ExecutionError;
    use crate::history_components::{AgentHistoryStore, HistoryError};
    use core_plugin::App;
    use resource_id_plugin::ResourcePlugin;
    use std::sync::{Barrier, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Clone, Default)]
    struct History(Arc<Mutex<Vec<HistoryRecord>>>);
    impl AgentHistoryStore for History {
        fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
            self.0.lock().unwrap().extend(records);
            Ok(())
        }
        fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }
    impl crate::block_storage::AgentBlockStorage for History {
        fn execute(
            &self,
            _: crate::block_storage::BlockStorageRequest,
        ) -> Result<crate::block_storage::StoredBlockResult, crate::block_storage::BlockStorageError>
        {
            Err(crate::block_storage::BlockStorageError::InvalidRequest(
                "fixture has no block backend".into(),
            ))
        }
    }
    fn id(value: &str) -> ResourceID {
        ResourceID::parse(value).unwrap()
    }
    fn sandbox(name: &str) -> SandboxValue {
        SandboxValue {
            resource_id: id(&format!("sandbox:local/{name}")),
            policy: if name == "second" {
                "{\"version\":2}"
            } else {
                "{\"version\":1}"
            }
            .into(),
        }
    }
    fn app(runtime: bool) -> (App, Entity, ResourceID) {
        let mut app = App::new();
        if runtime {
            app.add_plugin(app_runtime_plugin::RuntimePlugin::default())
                .add_plugin(async_runtime_plugin::AsyncRuntimePlugin);
        }
        app.add_plugin(ResourcePlugin);
        app.world_mut().register_library::<Agent>("agent").unwrap();
        app.world_mut()
            .register_resources::<ResourceSet<SandboxResource>>("sandbox")
            .unwrap();
        let agent_id = id("agent:local/test");
        let entity = app.world_mut().spawn();
        let database = AgentDatabaseHandle::new(History::default());
        let agent = Agent {
            resourceid: agent_id.clone(),
            info: AgentInfo {
                model: AgentImageModelConfig {
                    model: "test".into(),
                    temperature: None,
                    max_output_tokens: None,
                    top_p: None,
                    stop: Vec::new(),
                },
                project_root: PathBuf::from("/tmp/project"),
                home_root: PathBuf::from("/tmp/home"),
                image: id("image:local/test"),
                image_dependencies: Arc::from([
                    id("sandbox:local/first"),
                    id("sandbox:local/second"),
                    id("prompt:local/first"),
                    id("tool:local/first"),
                ]),
            },
            workspace: id("workspace:local/test"),
            sandbox: None,
            lua: Default::default(),
            state: AgentState::Idle,
            history: database.history(),
            database,
            import_bindings: Default::default(),
            mcl: Default::default(),
            execution: Default::default(),
            turn: None,
        };
        app.world_mut().insert_library(entity, agent).unwrap();
        let mut resources = ResourceSet::new();
        resources.insert(
            id("sandbox:local/first"),
            SandboxResource {
                alias: "first".into(),
                policy: "{\"version\":1}".into(),
            },
        );
        resources.insert(
            id("sandbox:local/second"),
            SandboxResource {
                alias: "second".into(),
                policy: "{\"version\":2}".into(),
            },
        );
        app.world_mut().insert_resources(entity, resources).unwrap();
        for (name, version) in [("first", 1), ("second", 2)] {
            app.world_mut()
                .agent_bind_import(
                    &agent_id,
                    name.into(),
                    ImportedValue::Sandbox(SandboxValue {
                        resource_id: id(&format!("sandbox:local/{name}")),
                        policy: format!("{{\"version\":{version}}}").into(),
                    }),
                )
                .unwrap();
        }
        (app, entity, agent_id)
    }
    fn wait(mut reply: oneshot::Receiver<Result<(), AgentError>>) -> Result<(), AgentError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match reply.try_recv() {
                Ok(result) => return result,
                Err(oneshot::error::TryRecvError::Closed) => panic!("cleanup reply lost"),
                Err(_) => {
                    assert!(Instant::now() < deadline, "cleanup timed out");
                    std::thread::yield_now();
                }
            }
        }
    }
    #[test]
    fn import_alias_grammar_is_shared_and_invalid_aliases_are_non_mutating() {
        let (mut app, entity, agent_id) = app(false);
        let source = id("prompt:local/first");
        let value = ImportedValue::Message {
            resource_id: source.clone(),
            messages: Arc::from([]),
        };
        let before = app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .import_bindings
            .clone();
        for alias in ["a.b", "-name", "", "has space", " name", "1name", "名字"] {
            assert_eq!(
                app.world()
                    .agent_validate_import(&agent_id, alias, InnerType::Message, &source)
                    .unwrap_err()
                    .kind,
                AgentErrorKind::InvalidDefinition
            );
            assert_eq!(
                app.world_mut()
                    .agent_bind_import(&agent_id, alias.into(), value.clone())
                    .unwrap_err()
                    .kind,
                AgentErrorKind::InvalidDefinition
            );
            assert_eq!(
                app.world()
                    .get_component::<Agent>(entity)
                    .unwrap()
                    .import_bindings,
                before
            );
        }
        for alias in ["review-flow", "_name", "Name_2"] {
            app.world()
                .agent_validate_import(&agent_id, alias, InnerType::Message, &source)
                .unwrap();
            app.world_mut()
                .agent_bind_import(&agent_id, alias.into(), value.clone())
                .unwrap();
            assert_eq!(
                app.world().agent_get_import(&agent_id, alias).unwrap(),
                BlockInner::Message(vec![])
            );
        }
    }

    #[test]
    fn declared_legacy_dependencies_cannot_be_imported_as_tools() {
        let (mut app, entity, agent_id) = app(false);
        let legacy = [id("skill:local/first"), id("hook:local/first")];
        app.world_mut()
            .get_component_mut::<Agent>(entity)
            .unwrap()
            .info
            .image_dependencies = Arc::from(legacy.clone());
        for source in legacy {
            assert_eq!(
                app.world()
                    .agent_validate_import(&agent_id, "legacy", InnerType::Tool, &source)
                    .unwrap_err()
                    .kind,
                AgentErrorKind::InvalidDefinition
            );
            assert_eq!(
                crate::resource_events::validate_resource_import(
                    app.world(),
                    entity,
                    &source,
                    "legacy",
                    Some(InnerType::Tool)
                )
                .unwrap_err()
                .kind,
                crate::resource_events::ResourceLoadErrorKind::InvalidRequest
            );
        }
        assert!(app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .import_bindings
            .get("legacy")
            .is_none());
    }

    #[test]
    fn bindings_reject_cross_kind_conflicts_and_rename_atomically() {
        let (mut app, entity, agent_id) = app(false);
        let message = ImportedValue::Message {
            resource_id: id("prompt:local/first"),
            messages: Arc::from([AgentMessage::System {
                content: "original".into(),
            }]),
        };
        assert_eq!(
            app.world_mut()
                .agent_bind_import(&agent_id, "first".into(), message.clone())
                .unwrap_err()
                .kind,
            AgentErrorKind::AliasConflict
        );
        app.world_mut()
            .agent_bind_import(&agent_id, "prompt".into(), message.clone())
            .unwrap();
        let old = app.world().agent_get_import(&agent_id, "prompt").unwrap();
        let before = app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .import_bindings
            .clone();
        assert_eq!(
            app.world_mut()
                .agent_bind_import(&agent_id, "second".into(), message.clone())
                .unwrap_err()
                .kind,
            AgentErrorKind::AliasConflict
        );
        assert_eq!(
            app.world()
                .get_component::<Agent>(entity)
                .unwrap()
                .import_bindings,
            before
        );
        let newer = ImportedValue::Message {
            resource_id: id("prompt:local/first"),
            messages: Arc::from([AgentMessage::System {
                content: "updated".into(),
            }]),
        };
        app.world_mut()
            .agent_bind_import(&agent_id, "renamed".into(), newer)
            .unwrap();
        assert!(app.world().agent_get_import(&agent_id, "prompt").is_err());
        assert_ne!(
            old,
            app.world().agent_get_import(&agent_id, "renamed").unwrap()
        );
        assert_eq!(
            old,
            BlockInner::Message(vec![AgentMessage::System {
                content: "original".into()
            }])
        );
        assert!(app
            .world_mut()
            .agent_bind_import(
                &agent_id,
                "invalid".into(),
                ImportedValue::Message {
                    resource_id: id("tool:local/first"),
                    messages: Arc::from([]),
                }
            )
            .is_err());
        assert!(app
            .world_mut()
            .agent_bind_import(
                &agent_id,
                "undeclared".into(),
                ImportedValue::Message {
                    resource_id: id("prompt:local/missing"),
                    messages: Arc::from([]),
                }
            )
            .is_err());
    }

    #[test]
    fn selected_sandbox_survives_reload_and_same_id_policy_change_is_busy() {
        let (mut app, entity, agent_id) = app(false);
        let first = sandbox("first");
        app.world_mut()
            .agent_switch_sandbox(&agent_id, Some(first.clone()))
            .unwrap();
        let activity = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        let newer = SandboxValue {
            resource_id: first.resource_id.clone(),
            policy: "new policy".into(),
        };
        app.world_mut()
            .get_component_mut::<ResourceSet<SandboxResource>>(entity)
            .unwrap()
            .get_mut(&first.resource_id)
            .unwrap()
            .policy = newer.policy.clone();
        app.world_mut()
            .agent_bind_import(
                &agent_id,
                "first".into(),
                ImportedValue::Sandbox(newer.clone()),
            )
            .unwrap();
        assert_eq!(
            app.world()
                .get_component::<Agent>(entity)
                .unwrap()
                .sandbox
                .as_ref(),
            Some(&first)
        );
        let snapshot_activity = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        assert_eq!(
            snapshot_activity.sandbox_snapshot.as_ref().unwrap().policy,
            first.policy
        );
        assert_eq!(
            app.world_mut()
                .agent_switch_sandbox(&agent_id, Some(newer.clone()))
                .unwrap_err()
                .kind,
            AgentErrorKind::ExecutionBusy
        );
        snapshot_activity.mark_completed().unwrap();
        activity.mark_completed().unwrap();
        app.world_mut()
            .agent_switch_sandbox(&agent_id, Some(newer.clone()))
            .unwrap();
        let updated = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        assert_eq!(
            updated.sandbox_snapshot.as_ref().unwrap().policy,
            newer.policy
        );
        updated.mark_completed().unwrap();
    }

    #[test]
    fn exact_identity_resolution_rejects_missing_wrong_type_and_duplicates() {
        let (mut app, entity, agent_id) = app(false);
        assert_eq!(app.world().agent_by_id(&agent_id).unwrap(), entity);
        assert_eq!(
            app.world()
                .agent_by_id(&id("tool:local/test"))
                .unwrap_err()
                .kind,
            AgentErrorKind::InvalidResourceID
        );
        assert_eq!(
            app.world()
                .agent_by_id(&id("agent:local/missing"))
                .unwrap_err()
                .kind,
            AgentErrorKind::AgentNotAlive
        );
        let (mut other_app, other_entity, _) = self::app(false);
        let duplicate = other_app
            .world_mut()
            .remove_component::<Agent>(other_entity)
            .unwrap();
        let duplicate_entity = app.world_mut().spawn();
        app.world_mut()
            .insert_component(duplicate_entity, duplicate);
        assert_eq!(
            app.world().agent_by_id(&agent_id).unwrap_err().kind,
            AgentErrorKind::DuplicateAgent
        );
        app.world_mut().despawn(duplicate_entity);
        app.world_mut().despawn(entity);
        assert_eq!(
            app.world().agent_by_id(&agent_id).unwrap_err().kind,
            AgentErrorKind::AgentNotAlive
        );
    }
    #[test]
    fn same_sandbox_configuration_is_idempotent_while_real_switch_is_busy() {
        let (mut app, entity, agent_id) = app(false);
        let first = sandbox("first");
        let second = sandbox("second");
        app.world_mut()
            .agent_switch_sandbox(&agent_id, Some(first.clone()))
            .unwrap();
        let activity = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        app.world_mut()
            .agent_switch_sandbox(&agent_id, Some(first.clone()))
            .unwrap();
        assert_eq!(
            app.world_mut()
                .agent_switch_sandbox(&agent_id, Some(second.clone()))
                .unwrap_err()
                .kind,
            AgentErrorKind::ExecutionBusy
        );
        assert_eq!(
            app.world()
                .get_component::<Agent>(entity)
                .unwrap()
                .sandbox
                .as_ref(),
            Some(&first)
        );
        assert_eq!(
            app.world_mut()
                .agent_switch_sandbox(&agent_id, None)
                .unwrap_err()
                .kind,
            AgentErrorKind::ExecutionBusy
        );
        let scope = app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .clone();
        let before = app
            .world()
            .get_component::<ResourceSet<SandboxResource>>(entity)
            .unwrap()
            .get(&first.resource_id)
            .unwrap()
            .policy
            .clone();
        let publish = scope.with_sandbox_switch(|| {
            app.world_mut()
                .get_component_mut::<ResourceSet<SandboxResource>>(entity)
                .unwrap()
                .get_mut(&first.resource_id)
                .unwrap()
                .policy = "changed".into();
            Ok(())
        });
        assert_eq!(publish, Err(ExecutionError::ExecutionBusy));
        assert_eq!(
            app.world()
                .get_component::<ResourceSet<SandboxResource>>(entity)
                .unwrap()
                .get(&first.resource_id)
                .unwrap()
                .policy,
            before
        );
        activity.mark_completed().unwrap();
        app.world_mut()
            .agent_switch_sandbox(&agent_id, Some(second))
            .unwrap();
    }
    #[test]
    fn snapshot_is_exact_and_owned_even_after_source_removal() {
        let (mut app, entity, agent_id) = app(false);
        let first = sandbox("first");
        app.world_mut()
            .agent_switch_sandbox(&agent_id, Some(first.clone()))
            .unwrap();
        let activity = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        let snapshot = activity.sandbox_snapshot.clone().unwrap();
        assert_eq!(snapshot.resource_id, first.resource_id);
        assert_eq!(&*snapshot.policy, "{\"version\":1}");
        app.world_mut()
            .get_component_mut::<ResourceSet<SandboxResource>>(entity)
            .unwrap()
            .remove(&first.resource_id);
        let retained = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        assert_eq!(
            retained.sandbox_snapshot.as_ref().unwrap().policy,
            snapshot.policy
        );
        retained.mark_completed().unwrap();
        assert_eq!(&*snapshot.policy, "{\"version\":1}");
        assert_eq!(
            app.world()
                .get_component::<Agent>(entity)
                .unwrap()
                .execution
                .activity_count(),
            1
        );
        activity.mark_completed().unwrap();
    }
    #[test]
    fn invalid_sandbox_switch_is_non_mutating_and_inference_does_not_block_switch() {
        let (mut app, entity, agent_id) = app(false);
        let inference = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Inference)
            .unwrap();
        assert!(inference.sandbox_snapshot.is_none());
        assert_eq!(
            app.world_mut()
                .agent_switch_sandbox(&agent_id, Some(sandbox("missing")))
                .unwrap_err()
                .kind,
            AgentErrorKind::SandboxNotLoaded
        );
        assert!(app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .sandbox
            .is_none());
        app.world_mut()
            .agent_switch_sandbox(&agent_id, Some(sandbox("first")))
            .unwrap();
        inference.mark_completed().unwrap();
    }
    #[test]
    fn cleanup_service_runs_from_plain_ecs_thread_and_waits_for_real_completion() {
        let (app, entity, agent_id) = app(true);
        assert!(tokio::runtime::Handle::try_current().is_err());
        let scope = app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .clone();
        let sandbox = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        sandbox.mark_started().unwrap();
        let inference = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Inference)
            .unwrap();
        let mut reply = app.world().agent_cleanup_tasks(&agent_id).unwrap();
        assert!(sandbox.cancel.is_cancelled());
        assert!(!inference.cancel.is_cancelled());
        assert_eq!(scope.admission(), ExecutionAdmission::SandboxPaused);
        assert!(matches!(
            reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        sandbox.mark_completed().unwrap();
        wait(reply).unwrap();
        assert_eq!(scope.admission(), ExecutionAdmission::Open);
        assert_eq!(scope.activity_count(), 1);
        inference.mark_completed().unwrap();
    }
    #[test]
    fn missing_async_runtime_does_not_pause_or_cancel_scope() {
        let (app, entity, agent_id) = app(false);
        let scope = app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .clone();
        let sandbox = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        assert_eq!(
            app.world().agent_cleanup_tasks(&agent_id).unwrap_err().kind,
            AgentErrorKind::CleanupFailed
        );
        assert_eq!(scope.admission(), ExecutionAdmission::Open);
        assert!(!sandbox.cancel.is_cancelled());
        sandbox.mark_completed().unwrap();
    }
    #[test]
    fn cleanup_failure_retains_registration_and_global_close_never_reopens() {
        let (app, entity, agent_id) = app(true);
        let scope = app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .clone();
        let sandbox = app
            .world()
            .agent_register_activity(&agent_id, ExecutionActivityKind::Sandbox)
            .unwrap();
        sandbox.mark_cleanup_failed(ExecutionError::CleanupFailed {
            activity_id: sandbox.id,
            message: "live pipe".into(),
        });
        assert_eq!(
            wait(app.world().agent_cleanup_tasks(&agent_id).unwrap())
                .unwrap_err()
                .kind,
            AgentErrorKind::CleanupFailed
        );
        assert_eq!(scope.activity_count(), 1);
        scope.request_close();
        sandbox.mark_completed().unwrap();
        wait(app.world().agent_cleanup_tasks(&agent_id).unwrap()).unwrap();
        assert_eq!(scope.admission(), ExecutionAdmission::Closed);
        assert_eq!(scope.activity_count(), 0);
    }
    #[test]
    fn registration_snapshot_and_switch_callbacks_share_the_same_lock() {
        for _ in 0..50 {
            let (mut app, entity, agent_id) = app(false);
            app.world_mut()
                .agent_switch_sandbox(&agent_id, Some(sandbox("first")))
                .unwrap();
            let scope = app
                .world()
                .get_component::<Agent>(entity)
                .unwrap()
                .execution
                .clone();
            let barrier = Arc::new(Barrier::new(2));
            let thread_barrier = barrier.clone();
            let thread = std::thread::spawn(move || {
                scope.register_with_snapshot(ExecutionActivityKind::Sandbox, || {
                    thread_barrier.wait();
                    Ok(Some(SandboxPolicySnapshot {
                        resource_id: id("sandbox:local/first"),
                        policy: "{\"version\":1}".into(),
                    }))
                })
            });
            barrier.wait();
            assert_eq!(
                app.world_mut()
                    .agent_switch_sandbox(&agent_id, Some(sandbox("second")))
                    .unwrap_err()
                    .kind,
                AgentErrorKind::ExecutionBusy
            );
            thread.join().unwrap().unwrap().mark_completed().unwrap();
        }
    }
    #[test]
    fn database_view_lookup_is_checked_and_history_shares_the_store() {
        let (app, entity, agent_id) = app(false);
        app.world().agent_block_storage(&agent_id).unwrap();
        assert!(app
            .world()
            .agent_block_storage(&id("agent:local/missing"))
            .is_err());
        let agent = app.world().get_component::<Agent>(entity).unwrap();
        let record = HistoryRecord {
            created_at_ms: 1,
            key: "shared-view".into(),
            source: "test".into(),
            turn: None,
            kind: "test".into(),
            content: "same store".into(),
            payload: serde_json::Value::Null,
        };
        agent.database.history().append(record.clone()).unwrap();
        assert_eq!(agent.history.list().unwrap(), vec![record]);
    }
    #[test]
    fn new_turn_updates_agent_and_history_append_uses_its_own_handle() {
        let (mut app, entity, agent_id) = app(false);
        let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
        let first = agent.new_turn();
        let second = agent.new_turn();
        assert_ne!(first, second);
        assert_eq!(agent.turn.as_ref(), Some(&second));
        let record = HistoryRecord {
            created_at_ms: 1,
            key: "record".into(),
            source: agent_id.to_string(),
            turn: Some(second),
            kind: "command".into(),
            content: "test".into(),
            payload: serde_json::Value::Null,
        };
        app.world()
            .agent_append_history_batch(&agent_id, vec![record.clone()])
            .unwrap();
        assert_eq!(
            app.world()
                .get_component::<Agent>(entity)
                .unwrap()
                .history
                .list()
                .unwrap(),
            vec![record]
        );
    }
}
