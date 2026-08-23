mod error;
mod events;
mod handler;
mod system;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use agent_plugin::{Agent, AgentResourceEntry};
use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AppAsyncExt;
use core_plugin::{App, Plugin, Resource};
use margatroid_types::{ResourceId, ToolCall};

pub use error::{ToolError, ToolErrorKind};
pub use events::{CancelToolTurn, ToolCallEvent, ToolRegisterRequest, ToolRegisterResponse};
pub use types::{
    AgentToolEnvironment, ResourceContent, ResourceMapEntry, ToolCallRequest, ToolTemplate,
};

use system::{
    cancel_tool_turn_system, tool_call_route_system, tool_message_cleanup_system,
    tool_register_system,
};

pub struct ToolPlugin {
    schedule: String,
    skill_root: Arc<PathBuf>,
    hook_root: Arc<PathBuf>,
    lua_root: Arc<PathBuf>,
    shell_root: Arc<PathBuf>,
    lua_limits: handler::lua::LuaExecutionLimits,
    shell_limits: handler::shell::ShellExecutionLimits,
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
            schedule: RuntimePlugin::UPDATE.to_owned(),
            skill_root: Arc::new(root.join("skills")),
            hook_root: Arc::new(root.join("hooks")),
            lua_root: Arc::new(root.join("tools")),
            shell_root: Arc::new(root.join("shells")),
            lua_limits: handler::lua::LuaExecutionLimits::default(),
            shell_limits: handler::shell::ShellExecutionLimits::default(),
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
            schedule: RuntimePlugin::UPDATE.to_owned(),
            skill_root: Arc::new(PathBuf::new()),
            hook_root: Arc::new(PathBuf::new()),
            lua_root: Arc::new(PathBuf::new()),
            shell_root: Arc::new(PathBuf::new()),
            lua_limits: handler::lua::LuaExecutionLimits::default(),
            shell_limits: handler::shell::ShellExecutionLimits::default(),
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
        app.world_mut().insert_resource(ToolPluginInstalled);
        app.world_mut().insert_resource(handler::skill::SkillRoots {
            home_root: self.skill_root.clone(),
        });
        app.world_mut().insert_resource(handler::hook::HookRoots {
            home_root: self.hook_root.clone(),
        });
        app.world_mut().insert_resource(handler::lua::LuaRoots {
            home_root: self.lua_root.clone(),
        });
        app.world_mut().insert_resource(self.lua_limits);
        app.world_mut().insert_resource(handler::lua::LuaHttpClient(
            reqwest::Client::builder()
                .build()
                .expect("Lua HTTP client could not be built"),
        ));
        app.world_mut().insert_resource(handler::shell::ShellRoots {
            home_root: self.shell_root.clone(),
        });
        app.world_mut().insert_resource(self.shell_limits);
        app.world_mut()
            .insert_resource(handler::shell::PersistentShells::default());
        app.add_system(&self.schedule, tool_register_system)
            .add_system(&self.schedule, handler::skill::skill_register_system)
            .add_system(&self.schedule, handler::hook::hook_register_system)
            .add_system(&self.schedule, handler::lua::lua_tool_register_system)
            .add_system(&self.schedule, handler::shell::shell_register_system)
            .add_system(&self.schedule, tool_call_route_system)
            .add_async_system(&self.schedule, handler::lua::execute_prepared_lua_tool)
            .add_system(&self.schedule, handler::lua::lua_task_result_system)
            .add_async_system(&self.schedule, handler::shell::execute_prepared_shell)
            .add_system(&self.schedule, handler::shell::shell_task_result_system)
            .add_system(&self.schedule, cancel_tool_turn_system)
            .add_system(&self.schedule, tool_message_cleanup_system);
    }
}

pub fn register_agent_resource(
    world: &mut core_plugin::World,
    agent: core_plugin::Entity,
    entry: ResourceMapEntry,
) -> Result<ResourceMapEntry, ToolError> {
    let tool_id = entry.tool_id.clone().ok_or_else(|| {
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
            tool_id,
            description: template.description,
            parameters: template.parameters,
        })
        .map_err(|error| ToolError::new(ToolErrorKind::DuplicateResource, error.to_string()))?;
    Ok(entry)
}

pub fn candidate_resource_entry(
    resource_id: ResourceId,
    alias: Option<String>,
    tool_id: ResourceId,
    mut template: ToolTemplate,
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
        tool_id: Some(tool_id),
        template: Some(template),
        content: None,
    })
}

pub fn resolve_agent_tool_definitions(
    world: &core_plugin::World,
    agent: core_plugin::Entity,
    resources: &[ResourceId],
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
