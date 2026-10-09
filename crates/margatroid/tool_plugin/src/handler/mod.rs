pub(crate) mod hook;
pub(crate) mod lua;
pub(crate) mod prompt;
pub(crate) mod pty;
pub(crate) mod sandbox;
pub(crate) mod skill;

use agent_plugin::Agent;
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_types::{MclEffectReply, Message};

use crate::{ToolCallRequest, ToolError};

#[cfg(test)]
mod library_tests {
    use super::*;
    use margatroid_types::ResourceRef;
    use std::path::{Path, PathBuf};

    #[test]
    fn packages_resolve_only_under_global_library_root() {
        type Resolver = fn(&Path, &ResourceRef) -> Result<PathBuf, ToolError>;
        let cases: [(&str, &str, &[&str], Resolver); 5] = [
            (
                "tool",
                "tools",
                &["tool.toml", "input.schema.json", "main.lua"],
                lua::find_lua_tool_package,
            ),
            ("skill", "skills", &["SKILL.md"], skill::find_skill_file),
            (
                "hook",
                "hooks",
                &["hook.toml", "input.schema.json"],
                hook::find_hook_package,
            ),
            (
                "sandbox",
                "sandboxes",
                &["policy.json"],
                sandbox::find_sandbox_package,
            ),
            (
                "prompt",
                "prompts",
                &["prompt.toml", "content.md"],
                prompt::find_prompt_package,
            ),
        ];
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("global");
        for (kind, collection, files, resolve) in cases {
            let id = ResourceRef::parse(&format!("{kind}:test/example:latest")).unwrap();
            for base in [root.join(".margatroid"), directory.path().join("image")] {
                let package = base.join(collection).join("test/example/latest");
                std::fs::create_dir_all(&package).unwrap();
                for file in files {
                    std::fs::write(package.join(file), "shadow").unwrap();
                }
            }
            assert!(resolve(&root, &id).is_err(), "{kind} must not use fallback");
            let package = root.join(collection).join("test/example/latest");
            std::fs::create_dir_all(&package).unwrap();
            for file in files {
                std::fs::write(package.join(file), "global").unwrap();
            }
            let expected = if kind == "skill" {
                package.join("SKILL.md")
            } else {
                package
            };
            assert_eq!(resolve(&root, &id).unwrap(), expected);
        }
    }
}

pub(crate) fn library_root(world: &World) -> Result<&std::path::Path, ToolError> {
    world
        .get_resource::<resource_library_plugin::ResourceLibraryRoot>()
        .map(|root| root.0.as_path())
        .ok_or_else(|| {
            ToolError::new(
                crate::ToolErrorKind::ToolEnvironmentMissing,
                "ResourceLibraryRoot is missing",
            )
        })
}

pub(crate) fn finish_tool_call(
    world: &mut World,
    request: ToolCallRequest,
    result: Result<String, ToolError>,
) {
    let pending = world
        .get_component_mut::<Agent>(request.agent)
        .and_then(|agent| {
            agent.tools.pending.remove(&(
                request.agent,
                request.turn_id.clone(),
                request.tool_call_id.clone(),
            ))
        });
    if pending.is_none() {
        return;
    }
    let failed = result.is_err();
    let content = result.unwrap_or_else(|error| error.to_string());
    world.send_event(MclEffectReply {
        id: request.effect_id,
        agent: request.agent,
        result: Ok(Message::Tool {
            resource_id: request.resource_id,
            tool_call_id: request.tool_call_id,
            content,
            failed,
        }),
        usage: None,
    });
}
