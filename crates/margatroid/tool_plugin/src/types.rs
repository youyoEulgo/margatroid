use std::sync::Arc;

use agent_plugin::ToolExecutor;
use core_plugin::Entity;
use margatroid_types::ResourceRef;

use crate::ToolError;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolTemplate {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResourceContent {
    Tool { script_path: std::path::PathBuf },
    Skill { content_path: std::path::PathBuf },
    Hook,
    Prompt { role: String, content: Arc<str> },
    Sandbox { policy: Arc<str> },
}

impl ToolTemplate {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Result<Self, ToolError> {
        let template = Self {
            name: name.into(),
            description: description.into(),
            parameters,
        };
        validate_template(&template)?;
        Ok(template)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceMapEntry {
    pub resource_id: ResourceRef,
    pub resource_name: String,
    pub alias: Option<String>,
    pub executor: Option<ToolExecutor>,
    pub template: Option<ToolTemplate>,
    pub content: Option<ResourceContent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub effect_id: String,
    pub turn_id: String,
    pub agent: Entity,
    pub executor: ToolExecutor,
    pub resource_id: ResourceRef,
    pub tool_call_id: String,
    pub arguments: String,
}

pub(crate) fn validate_template(template: &ToolTemplate) -> Result<(), ToolError> {
    if template.description.trim().is_empty() || !template.parameters.is_object() {
        return Err(ToolError::new(
            crate::ToolErrorKind::InvalidDefinition,
            "tool template is invalid",
        ));
    }
    Ok(())
}
