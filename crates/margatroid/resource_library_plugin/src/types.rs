use std::collections::HashSet;

use core_plugin::{Component, Entity};

use crate::{HookResource, PromptResource, SandboxResource, SkillResource, ToolResource};

#[derive(Clone, Debug, Default)]
pub struct ResourceOwners {
    agents: HashSet<Entity>,
}

impl ResourceOwners {
    pub fn contains(&self, agent: Entity) -> bool {
        self.agents.contains(&agent)
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub(crate) fn acquire(&mut self, agent: Entity) {
        self.agents.insert(agent);
    }

    pub(crate) fn release(&mut self, agent: Entity) {
        self.agents.remove(&agent);
    }
}

impl Component for ResourceOwners {}

#[derive(Clone, Debug)]
pub enum ResourceDefinition {
    Tool(ToolResource),
    Skill(SkillResource),
    Prompt(PromptResource),
    Sandbox(SandboxResource),
    Hook(HookResource),
}

impl ResourceDefinition {
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::Tool(_) => "tool",
            Self::Skill(_) => "skill",
            Self::Prompt(_) => "prompt",
            Self::Sandbox(_) => "sandbox",
            Self::Hook(_) => "hook",
        }
    }
}
