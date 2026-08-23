use std::path::{Path, PathBuf};
use std::sync::Arc;

use core_plugin::{Component, Entity};
use margatroid_types::ResourceId;

use crate::ToolError;

#[derive(Clone)]
pub struct AgentToolEnvironment {
    project_root: Arc<PathBuf>,
    image_root: Arc<PathBuf>,
}

impl AgentToolEnvironment {
    pub fn new(project_root: impl Into<PathBuf>, image_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: Arc::new(project_root.into()),
            image_root: Arc::new(image_root.into()),
        }
    }

    pub fn project_root(&self) -> &Path {
        self.project_root.as_path()
    }

    pub fn image_root(&self) -> &Path {
        self.image_root.as_path()
    }
}

impl Component for AgentToolEnvironment {}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolTemplate {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResourceContent {
    Prompt { role: String, content: Arc<str> },
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
    pub resource_id: ResourceId,
    pub resource_name: String,
    pub alias: Option<String>,
    pub tool_id: Option<ResourceId>,
    pub template: Option<ToolTemplate>,
    pub content: Option<ResourceContent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub turn_id: String,
    pub agent: Entity,
    pub tool_id: ResourceId,
    pub resource_id: ResourceId,
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
