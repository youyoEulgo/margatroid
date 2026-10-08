mod error;
mod events;
mod handler;
mod system;
mod types;

use std::path::PathBuf;

use agent_plugin::{Agent, AgentResourceEntry, ToolExecutor};
use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AppAsyncExt;
use core_plugin::{App, Plugin, Resource};
use margatroid_types::{ResourceRef, ToolCall};

pub use error::{ToolError, ToolErrorKind};
pub use events::{CancelToolTurn, ToolCallEvent, ToolRegisterRequest, ToolRegisterResponse};
pub use types::{ResourceContent, ResourceMapEntry, ToolCallRequest, ToolTemplate};

use system::{cancel_tool_turn_system, tool_call_route_system, tool_register_system};

pub struct ToolPlugin {
    root: Option<PathBuf>,
    schedule: String,
    lua_limits: handler::lua::LuaExecutionLimits,
}

impl ToolPlugin {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ToolError> {
        let root = root.into();
        if !root.is_absolute()
            || root
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "tool root must be absolute and cannot contain parent traversal",
            ));
        }
        Ok(Self {
            root: Some(root),
            schedule: RuntimePlugin::UPDATE.to_owned(),
            lua_limits: handler::lua::LuaExecutionLimits::default(),
        })
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for ToolPlugin {
    fn default() -> Self {
        Self {
            root: None,
            schedule: RuntimePlugin::UPDATE.to_owned(),
            lua_limits: handler::lua::LuaExecutionLimits::default(),
        }
    }
}

pub struct ToolPluginInstalled;
impl Resource for ToolPluginInstalled {}

impl Plugin for ToolPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            ToolError::new(
                ToolErrorKind::ToolPluginMissing,
                "RuntimePlugin is required",
            )
            .panic();
        }
        if app.world().contains_resource::<ToolPluginInstalled>() {
            ToolError::new(
                ToolErrorKind::ToolAlreadyRegistered,
                "ToolPlugin is already installed",
            )
            .panic();
        }
        if !app.contains_schedule(&self.schedule) {
            ToolError::new(
                ToolErrorKind::InvalidRequest,
                "ToolPlugin schedule does not exist",
            )
            .panic();
        }
        let installed_root = app
            .world()
            .get_resource::<resource_library_plugin::ResourceLibraryRoot>()
            .map(|root| root.0.clone());
        if let (Some(requested), Some(installed)) = (&self.root, &installed_root) {
            assert_eq!(
                requested, installed,
                "ToolPlugin root conflicts with ResourceLibraryRoot"
            );
        }
        let root = self.root.or(installed_root).unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("HOME").expect("HOME or ResourceLibraryRoot is required"),
            )
            .join(".margatroid")
        });
        assert!(
            root.is_absolute()
                && !root
                    .components()
                    .any(|part| part == std::path::Component::ParentDir),
            "ResourceLibraryRoot must be absolute and cannot contain parent traversal"
        );
        if !app
            .world()
            .contains_resource::<resource_library_plugin::ResourceLibraryRoot>()
        {
            app.world_mut()
                .insert_resource(resource_library_plugin::ResourceLibraryRoot(root));
        }
        app.world_mut().insert_resource(ToolPluginInstalled);
        app.world_mut().insert_resource(self.lua_limits);
        app.add_system(&self.schedule, tool_register_system)
            .add_system(&self.schedule, handler::skill::skill_register_system)
            .add_system(&self.schedule, handler::hook::hook_register_system)
            .add_system(&self.schedule, handler::sandbox::sandbox_register_system)
            .add_system(&self.schedule, handler::prompt::prompt_register_system)
            .add_system(&self.schedule, handler::lua::lua_tool_register_system)
            .add_system(&self.schedule, tool_call_route_system)
            .add_async_system(&self.schedule, handler::lua::execute_prepared_lua_tool)
            .add_system(&self.schedule, handler::lua::lua_task_result_system)
            .add_system(&self.schedule, cancel_tool_turn_system);
    }
}

pub fn register_agent_resource(
    world: &mut core_plugin::World,
    agent: core_plugin::Entity,
    entry: ResourceMapEntry,
) -> Result<ResourceMapEntry, ToolError> {
    let executor = entry.executor.ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "resource mapping is not executable",
        )
    })?;
    let template = entry.template.clone().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "executable resource has no tool template",
        )
    })?;
    let agent_state = world.get_component_mut::<Agent>(agent).ok_or_else(|| {
        ToolError::new(ToolErrorKind::AgentNotAlive, "Agent component is missing")
    })?;
    agent_state
        .resources
        .register_tool(AgentResourceEntry {
            resource_id: entry.resource_id.clone(),
            resource_name: entry.resource_name.clone(),
            executor,
            description: template.description,
            parameters: template.parameters,
        })
        .map_err(|error| ToolError::new(ToolErrorKind::DuplicateResource, error.to_string()))?;
    Ok(entry)
}

pub fn candidate_resource_entry(
    resource_id: ResourceRef,
    alias: Option<String>,
    executor: ToolExecutor,
    template: ToolTemplate,
) -> Result<ResourceMapEntry, ToolError> {
    candidate_resource_entry_with_content(resource_id, alias, executor, template, None)
}

/// The same candidate entry, carrying content that later stages need — a
/// sandbox's policy — alongside the template.
pub fn candidate_resource_entry_with_content(
    resource_id: ResourceRef,
    alias: Option<String>,
    executor: ToolExecutor,
    mut template: ToolTemplate,
    content: Option<ResourceContent>,
) -> Result<ResourceMapEntry, ToolError> {
    let resource_name = alias.clone().unwrap_or_else(|| resource_id.to_string());
    if resource_name.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "resource name is empty",
        ));
    }
    template.name = resource_name.clone();
    types::validate_template(&template)?;
    Ok(ResourceMapEntry {
        resource_id,
        resource_name,
        alias,
        executor: Some(executor),
        template: Some(template),
        content,
    })
}

pub fn resolve_agent_tool_definitions(
    world: &core_plugin::World,
    agent: core_plugin::Entity,
    resources: &[ResourceRef],
) -> Result<Vec<margatroid_types::ToolDefinition>, ToolError> {
    if resources.is_empty() {
        return Ok(Vec::new());
    }
    let agent_state = world.get_component::<Agent>(agent).ok_or_else(|| {
        ToolError::new(ToolErrorKind::AgentNotAlive, "Agent component is missing")
    })?;
    let mut definitions = Vec::with_capacity(resources.len());
    for resource_id in resources {
        let entries = agent_state.resources.tools_by_resource(resource_id);
        if entries.len() != 1 {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "visible resource is not registered exactly once",
            ));
        }
        let entry = entries[0];
        definitions.push(margatroid_types::ToolDefinition {
            name: entry.resource_name.clone(),
            description: entry.description.clone(),
            input_schema: entry.parameters.clone(),
        });
    }
    Ok(definitions)
}

pub fn validate_agent_tool_calls(
    world: &core_plugin::World,
    agent: core_plugin::Entity,
    calls: &[ToolCall],
) -> Result<(), ToolError> {
    let agent_state = world.get_component::<Agent>(agent).ok_or_else(|| {
        ToolError::new(ToolErrorKind::AgentNotAlive, "Agent component is missing")
    })?;
    for call in calls {
        if agent_state
            .resources
            .tool_by_name(&call.tool_name)
            .is_none()
        {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "tool call is not registered for this Agent",
            ));
        }
    }
    Ok(())
}

pub use handler::lua::{run_lua_tool, LuaExecutionLimits, LuaToolRunRequest};
