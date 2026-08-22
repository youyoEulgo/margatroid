use std::fmt;
use std::path::{Component, PathBuf};

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::{App, Plugin, Resource, World};
use hook_plugin::{HookPlugin, HookRegisterRequest, HookRegisterResponse};
use lua_plugin::{LuaPlugin, LuaToolRegisterRequest, LuaToolRegisterResponse};
use margatroid_types::ResourceId;
use shell_plugin::{ShellPlugin, ShellRegisterRequest, ShellRegisterResponse};
use skill_plugin::{SkillPlugin, SkillRegisterRequest, SkillRegisterResponse};
use tool_plugin::{
    candidate_resource_entry, AgentResourceRegisterRequest, AgentResourceRegisterResponse,
    ToolCallRequest, ToolCallResponse, ToolError, ToolErrorKind, ToolTemplate,
};

const HOOK_TOOL_ID: &str = "tool:builtin/hook:latest";
const HOOK_TOOL_DESCRIPTION: &str =
    "Invoke a named hook. The Agent Base Lua loop dispatches to the matching hook by name.";

pub struct BuiltinToolPlugin {
    skill_plugin: SkillPlugin,
    hook_plugin: HookPlugin,
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
        let hook_plugin = HookPlugin::open(data_root.join("hooks"))
            .map_err(|_| child_plugin_error("HookPlugin"))?;
        let lua_plugin = LuaPlugin::open(data_root.join("tools"))
            .map_err(|_| child_plugin_error("LuaPlugin"))?;
        let shell_plugin = ShellPlugin::open(data_root.join("shells"))
            .map_err(|_| child_plugin_error("ShellPlugin"))?;
        Ok(Self {
            skill_plugin,
            hook_plugin,
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
            .add_plugin(self.hook_plugin)
            .add_plugin(self.lua_plugin)
            .add_plugin(self.shell_plugin)
            .add_system(RuntimePlugin::UPDATE, builtin_resource_register_system)
            .add_system(RuntimePlugin::UPDATE, collect_builtin_registration_system)
            .add_system(RuntimePlugin::UPDATE, hook_tool_call_system);
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
        .event_reader::<AgentResourceRegisterRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        if request.id.is_empty() {
            world.send_event(AgentResourceRegisterResponse {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                alias: request.alias,
                result: Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "resource registration request is invalid",
                )),
            });
            continue;
        }
        if request.resource_id.to_string() == HOOK_TOOL_ID {
            let result = candidate_resource_entry(
                request.resource_id.clone(),
                request.alias.clone(),
                ResourceId::parse(HOOK_TOOL_ID).expect("built-in Hook tool ID must be valid"),
                ToolTemplate::new(
                    request.resource_id.to_string(),
                    HOOK_TOOL_DESCRIPTION,
                    serde_json::json!({"type":"object"}),
                )
                .expect("built-in Hook tool template must be valid"),
            );
            world.send_event(AgentResourceRegisterResponse {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                alias: request.alias,
                result,
            });
            continue;
        }
        if request.resource_id.resource_type() == "tool" && request.resource_id.scope() == "builtin"
        {
            world.send_event(AgentResourceRegisterResponse {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                alias: request.alias,
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
                alias: request.alias,
            }),
            "hook" => world.send_event(HookRegisterRequest {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                alias: request.alias,
            }),
            "tool" => world.send_event(LuaToolRegisterRequest {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                alias: request.alias,
            }),
            "shell" => world.send_event(ShellRegisterRequest {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                alias: request.alias,
            }),
            _ => world.send_event(AgentResourceRegisterResponse {
                id: request.id,
                agent: request.agent,
                resource_id: request.resource_id,
                alias: request.alias,
                result: Err(ToolError::new(
                    ToolErrorKind::ProviderMissing,
                    "resource type has no built-in executor",
                )),
            }),
        }
    }
}

fn hook_tool_call_system(world: &mut World) {
    let hook_tool_id =
        ResourceId::parse(HOOK_TOOL_ID).expect("built-in Hook tool ID must be valid");
    let calls = world
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .cloned()
        .filter(|call| call.tool_id == hook_tool_id)
        .collect::<Vec<_>>();
    for call in calls {
        world.send_event(ToolCallResponse {
            turn_id: call.turn_id,
            agent: call.agent,
            tool_call_id: call.tool_call_id,
            result: Ok(String::new()),
        });
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
                response.alias,
            )
        })
        .collect::<Vec<_>>();
    let hook = world
        .event_reader::<HookRegisterResponse>()
        .into_iter()
        .cloned()
        .map(|response| {
            (
                response.id,
                response.agent,
                response.resource_id,
                response.result,
                response.alias,
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
                response.alias,
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
                response.alias,
            )
        })
        .collect::<Vec<_>>();
    for (id, agent, resource_id, result, alias) in
        skill.into_iter().chain(hook).chain(lua).chain(shell)
    {
        world.send_event(AgentResourceRegisterResponse {
            id,
            agent,
            resource_id,
            alias,
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
