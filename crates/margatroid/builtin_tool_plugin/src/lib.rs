use std::fmt;
use std::path::{Component, PathBuf};

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::{App, Plugin, Resource, World};
use lua_plugin::{LuaPlugin, LuaToolRegisterRequest, LuaToolRegisterResponse};
use shell_plugin::{ShellPlugin, ShellRegisterRequest, ShellRegisterResponse};
use skill_plugin::{SkillPlugin, SkillRegisterRequest, SkillRegisterResponse};
use tool_plugin::{AgentToolRegisterRequest, AgentToolRegisterResponse, ToolError, ToolErrorKind};
use workflow_plugin::{WorkflowPlugin, WorkflowRegisterRequest, WorkflowRegisterResponse};

pub struct BuiltinToolPlugin {
    skill_plugin: SkillPlugin,
    workflow_plugin: WorkflowPlugin,
    lua_plugin: LuaPlugin,
    shell_plugin: ShellPlugin,
}

impl BuiltinToolPlugin {
    pub fn open(data_root: impl Into<PathBuf>) -> Result<Self, BuiltinToolError> {
        let data_root = normalize_root(data_root.into()).ok_or_else(|| {
            BuiltinToolError::new(
                BuiltinToolErrorKind::InvalidRoot,
                "built-in tool data root must be absolute and cannot contain parent traversal",
            )
        })?;
        let skill_plugin = SkillPlugin::open(data_root.join("skills"))
            .map_err(|_| child_plugin_error("SkillPlugin"))?;
        let workflow_plugin = WorkflowPlugin::open(data_root.join("workflows"))
            .map_err(|_| child_plugin_error("WorkflowPlugin"))?;
        let lua_plugin = LuaPlugin::open(data_root.join("tools"))
            .map_err(|_| child_plugin_error("LuaPlugin"))?;
        let shell_plugin = ShellPlugin::open(data_root.join("shells"))
            .map_err(|_| child_plugin_error("ShellPlugin"))?;
        Ok(Self {
            skill_plugin,
            workflow_plugin,
            lua_plugin,
            shell_plugin,
        })
    }
}

impl Plugin for BuiltinToolPlugin {
    fn build(self, app: &mut App) {
        if app
            .world()
            .contains_resource::<BuiltinToolPluginInstalled>()
        {
            panic!("BuiltinToolPlugin is already installed");
        }
        app.world_mut().insert_resource(BuiltinToolPluginInstalled);
        app.add_plugin(self.skill_plugin)
            .add_plugin(self.workflow_plugin)
            .add_plugin(self.lua_plugin)
            .add_plugin(self.shell_plugin)
            .add_system(RuntimePlugin::UPDATE, builtin_resource_register_system)
            .add_system(RuntimePlugin::UPDATE, collect_builtin_registration_system);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinToolErrorKind {
    InvalidRoot,
    ChildPluginInvalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinToolError {
    kind: BuiltinToolErrorKind,
    message: String,
}

impl BuiltinToolError {
    fn new(kind: BuiltinToolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> BuiltinToolErrorKind {
        self.kind
    }
}

impl fmt::Display for BuiltinToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for BuiltinToolError {}

struct BuiltinToolPluginInstalled;
impl Resource for BuiltinToolPluginInstalled {}

fn builtin_resource_register_system(world: &mut World) {
    let requests = world
        .event_reader::<AgentToolRegisterRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        if request.id.is_empty()
            || request.resource_id.resource_type() == "tool"
                && request.resource_id.scope() == "builtin"
        {
            world.send_event(AgentToolRegisterResponse {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                result: Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "built-in executors cannot be registered as visible resources",
                )),
            });
            continue;
        }
        match request.resource_id.resource_type() {
            "skill" => world.send_event(SkillRegisterRequest {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
            }),
            "workflow" => world.send_event(WorkflowRegisterRequest {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
            }),
            "tool" => world.send_event(LuaToolRegisterRequest {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
            }),
            "shell" => world.send_event(ShellRegisterRequest {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
            }),
            _ => world.send_event(AgentToolRegisterResponse {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                result: Err(ToolError::new(
                    ToolErrorKind::ProviderMissing,
                    "resource type has no built-in executor",
                )),
            }),
        }
    }
}

fn collect_builtin_registration_system(world: &mut World) {
    let skill = world
        .event_reader::<SkillRegisterResponse>()
        .into_iter()
        .cloned()
        .map(|response| {
            (
                response.id,
                response.agent,
                response.resource_id,
                response.result,
            )
        })
        .collect::<Vec<_>>();
    let workflow = world
        .event_reader::<WorkflowRegisterResponse>()
        .into_iter()
        .cloned()
        .map(|response| {
            (
                response.id,
                response.agent,
                response.resource_id,
                response.result,
            )
        })
        .collect::<Vec<_>>();
    let lua = world
        .event_reader::<LuaToolRegisterResponse>()
        .into_iter()
        .cloned()
        .map(|response| {
            (
                response.id,
                response.agent,
                response.resource_id,
                response.result,
            )
        })
        .collect::<Vec<_>>();
    let shell = world
        .event_reader::<ShellRegisterResponse>()
        .into_iter()
        .cloned()
        .map(|response| {
            (
                response.id,
                response.agent,
                response.resource_id,
                response.result,
            )
        })
        .collect::<Vec<_>>();
    for (id, agent, resource_id, result) in
        skill.into_iter().chain(workflow).chain(lua).chain(shell)
    {
        world.send_event(AgentToolRegisterResponse {
            id,
            agent,
            resource_id,
            result,
        });
    }
}

fn child_plugin_error(name: &str) -> BuiltinToolError {
    BuiltinToolError::new(
        BuiltinToolErrorKind::ChildPluginInvalid,
        format!("{name} could not be opened"),
    )
}

fn normalize_root(path: PathBuf) -> Option<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            normalized.push(component.as_os_str());
        }
    }
    Some(normalized)
}
