use crate::agent_components::{Agent, ImportedValue, WorldAgentExt};
use crate::block_types::InnerType;
use crate::execution_components::ExecutionActivity;
use crate::resource_components::{PromptResource, SandboxResource, ToolResource};
use core_plugin::{Entity, Event};
use resource_id_plugin::ResourceID;
use std::fmt;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceLoadErrorKind {
    InvalidRequest,
    InvalidRoot,
    InvalidResourceID,
    NotFound,
    InvalidFileType,
    SymlinkNotAllowed,
    LimitExceeded,
    ReadFailed,
    DecodeFailed,
    InvalidDefinition,
    TargetNotAlive,
    MountFailed,
    ExecutionBusy,
    TaskFailed,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceLoadError {
    pub kind: ResourceLoadErrorKind,
    pub message: String,
}
impl ResourceLoadError {
    pub fn new(kind: ResourceLoadErrorKind, message: impl Into<String>) -> Self {
        let message: String = message.into().chars().take(512).collect();
        Self { kind, message }
    }
    pub fn kind(&self) -> ResourceLoadErrorKind {
        self.kind
    }
}
impl fmt::Display for ResourceLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}:{}", self.kind, self.message)
    }
}
impl std::error::Error for ResourceLoadError {}
pub type ResourceLoadReply = Arc<Mutex<Option<oneshot::Sender<Result<(), ResourceLoadError>>>>>;

pub fn validate_resource_import(
    world: &core_plugin::World,
    target: Entity,
    resource_id: &ResourceID,
    alias: &str,
    kind: Option<InnerType>,
) -> Result<(), ResourceLoadError> {
    let Some(kind) = kind else {
        return Ok(());
    };
    let matches = match kind {
        InnerType::Tool => resource_id.resource_type() == "tool",
        InnerType::Message => resource_id.resource_type() == "prompt",
        InnerType::Sandbox => resource_id.resource_type() == "sandbox",
    };
    if !matches {
        return Err(ResourceLoadError::new(
            ResourceLoadErrorKind::InvalidRequest,
            "import declaration does not match resource type",
        ));
    }
    let agent = world.get_component::<Agent>(target).ok_or_else(|| {
        ResourceLoadError::new(
            ResourceLoadErrorKind::InvalidRequest,
            "import target is not an Agent",
        )
    })?;
    if world
        .agent_by_id(&agent.resourceid)
        .map_err(|e| ResourceLoadError::new(ResourceLoadErrorKind::InvalidRequest, e.to_string()))?
        != target
    {
        return Err(ResourceLoadError::new(
            ResourceLoadErrorKind::InvalidRequest,
            "import target instance mismatch",
        ));
    }
    world
        .agent_validate_import(&agent.resourceid, alias, kind, resource_id)
        .map_err(|e| ResourceLoadError::new(ResourceLoadErrorKind::InvalidRequest, e.to_string()))
}

pub fn mount_resource_import<T: Clone + Send + Sync + 'static>(
    world: &mut core_plugin::World,
    target: Entity,
    resource_id: &ResourceID,
    alias: &str,
    kind: Option<InnerType>,
    resource: T,
    imported: Option<ImportedValue>,
) -> Result<(), ResourceLoadError> {
    use resource_id_plugin::{ResourceSet, WorldResourceExt};
    validate_resource_import(world, target, resource_id, alias, kind)?;
    let mut set = world
        .get_component::<ResourceSet<T>>(target)
        .cloned()
        .unwrap_or_default();
    set.insert(resource_id.clone(), resource);
    let registration = world
        .get_resource::<resource_id_plugin::ResourceTypeRegistry>()
        .and_then(|registry| registry.get(resource_id.resource_type()));
    if !world.contains_resource::<resource_id_plugin::ResourcePluginInstalled>()
        || registration.is_none_or(|entry| {
            entry.component_type != std::any::TypeId::of::<ResourceSet<T>>()
                || entry.kind != resource_id_plugin::ResourceComponentKind::Resources
        })
        || set
            .resource_ids()
            .any(|id| id.resource_type() != resource_id.resource_type())
    {
        return Err(ResourceLoadError::new(
            ResourceLoadErrorKind::MountFailed,
            "resource set registration or address mismatch",
        ));
    }
    let previous = if kind.is_some() {
        let agent = world.get_component::<Agent>(target).unwrap();
        let id = agent.resourceid.clone();
        let previous = agent.import_bindings.clone();
        let value = imported.ok_or_else(|| {
            ResourceLoadError::new(
                ResourceLoadErrorKind::InvalidRequest,
                "missing typed import snapshot",
            )
        })?;
        if value.resource_id() != resource_id || Some(value.inner_type()) != kind {
            return Err(ResourceLoadError::new(
                ResourceLoadErrorKind::InvalidRequest,
                "import snapshot does not match requested resource and kind",
            ));
        }
        world
            .agent_bind_import(&id, alias.to_owned(), value)
            .map_err(|e| {
                ResourceLoadError::new(ResourceLoadErrorKind::InvalidRequest, e.to_string())
            })?;
        Some(previous)
    } else {
        None
    };
    if let Err(error) = world.insert_resources(target, set) {
        if let Some(previous) = previous {
            world
                .get_component_mut::<Agent>(target)
                .unwrap()
                .import_bindings = previous;
        }
        return Err(ResourceLoadError::new(
            ResourceLoadErrorKind::MountFailed,
            error.to_string(),
        ));
    }
    Ok(())
}

macro_rules! load_events {
    ($request:ident, $result:ident, $output:ident, $error:ident, $resource:ty) => {
        #[derive(Clone, Debug)]
        pub struct $request {
            pub id: String,
            pub target: Entity,
            pub resource_id: ResourceID,
            pub alias: String,
            pub import_kind: Option<InnerType>,
            pub cancel: CancellationToken,
            pub reply: ResourceLoadReply,
            pub activity: Option<Arc<ExecutionActivity>>,
        }
        impl Event for $request {}
        #[derive(Clone, Debug)]
        pub struct $result {
            pub id: String,
            pub target: Entity,
            pub resource_id: ResourceID,
            pub result: Result<(), ResourceLoadError>,
        }
        impl Event for $result {}
        #[derive(Clone, Debug)]
        pub struct $output {
            pub request: $request,
            pub result: Result<$resource, ResourceLoadError>,
        }
        #[derive(Clone, Debug)]
        pub struct $error {
            pub request: Option<$request>,
            pub message: String,
        }
        impl From<async_runtime_plugin::AsyncTaskError> for $error {
            fn from(source: async_runtime_plugin::AsyncTaskError) -> Self {
                Self {
                    request: None,
                    message: source.to_string(),
                }
            }
        }
    };
}
load_events!(
    LoadToolResource,
    LoadToolResourceResult,
    ToolResourceReadOutput,
    ToolResourceTaskError,
    ToolResource
);
load_events!(
    LoadPromptResource,
    LoadPromptResourceResult,
    PromptResourceReadOutput,
    PromptResourceTaskError,
    PromptResource
);
load_events!(
    LoadSandboxResource,
    LoadSandboxResourceResult,
    SandboxResourceReadOutput,
    SandboxResourceTaskError,
    SandboxResource
);
