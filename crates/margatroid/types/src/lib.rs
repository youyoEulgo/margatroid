use std::fmt;
use std::path::PathBuf;

use core_plugin::{Entity, Event};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceNameError {
    Empty,
    InvalidScope,
    InvalidName,
    InvalidCharacter,
}

impl fmt::Display for ResourceNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("resource name cannot be empty"),
            Self::InvalidScope => formatter.write_str("resource scope is invalid"),
            Self::InvalidName => formatter.write_str("resource name is invalid"),
            Self::InvalidCharacter => {
                formatter.write_str("resource name contains an invalid character")
            }
        }
    }
}

impl std::error::Error for ResourceNameError {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceName {
    scope: String,
    name: String,
}

impl ResourceName {
    pub fn new(value: impl Into<String>) -> Result<Self, ResourceNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ResourceNameError::Empty);
        }

        let mut parts = value.split('/');
        let scope = parts.next().ok_or(ResourceNameError::InvalidScope)?;
        let name = parts.next().ok_or(ResourceNameError::InvalidName)?;
        if parts.next().is_some() {
            return Err(ResourceNameError::InvalidName);
        }
        validate_part(scope).map_err(|error| match error {
            ResourceNameError::InvalidName => ResourceNameError::InvalidScope,
            error => error,
        })?;
        validate_part(name)?;

        Ok(Self {
            scope: scope.to_owned(),
            name: name.to_owned(),
        })
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for ResourceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.scope, self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceRefError {
    EmptyProvider,
    InvalidProvider,
}

impl fmt::Display for ResourceRefError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyProvider => formatter.write_str("resource provider cannot be empty"),
            Self::InvalidProvider => formatter.write_str("resource provider is invalid"),
        }
    }
}

impl std::error::Error for ResourceRefError {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceRef {
    provider: String,
    name: ResourceName,
}

impl ResourceRef {
    pub fn new(provider: impl Into<String>, name: ResourceName) -> Result<Self, ResourceRefError> {
        let provider = provider.into();
        if provider.is_empty() {
            return Err(ResourceRefError::EmptyProvider);
        }
        if !provider.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        }) {
            return Err(ResourceRefError::InvalidProvider);
        }
        Ok(Self { provider, name })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn name(&self) -> &ResourceName {
        &self.name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentImageReferenceError {
    InvalidName,
    InvalidTag,
}

impl fmt::Display for AgentImageReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => formatter.write_str("agent image name is invalid"),
            Self::InvalidTag => formatter.write_str("agent image tag is invalid"),
        }
    }
}

impl std::error::Error for AgentImageReferenceError {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentImageReference {
    resource: ResourceName,
    tag: String,
}

impl AgentImageReference {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentImageReferenceError> {
        let value = value.into();
        let (resource, tag) = match value.split_once(':') {
            Some((resource, tag)) => (resource, tag),
            None => (value.as_str(), "latest"),
        };
        let resource =
            ResourceName::new(resource).map_err(|_| AgentImageReferenceError::InvalidName)?;
        validate_tag(tag)?;
        Ok(Self {
            resource,
            tag: tag.to_owned(),
        })
    }

    pub fn resource(&self) -> &ResourceName {
        &self.resource
    }

    pub fn scope(&self) -> &str {
        self.resource.scope()
    }

    pub fn name(&self) -> &str {
        self.resource.name()
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }
}

impl fmt::Display for AgentImageReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.resource, self.tag)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceAgentDefinition {
    pub name: String,
    pub image: AgentImageReference,
    pub resources: Vec<ResourceRef>,
    pub disable_resources: Vec<ResourceRef>,
    pub memory_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceDefinition {
    pub name: String,
    pub project_root: PathBuf,
    pub manager: String,
    pub agents: Vec<WorkspaceAgentDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceReference {
    pub name: String,
    pub project_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartWorkspace {
    pub id: String,
    pub definition: WorkspaceDefinition,
}

impl Event for StartWorkspace {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteAgentMessage {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub agent: Option<String>,
    pub message: Message,
    pub tool_calls: Vec<ToolCall>,
}

impl Event for RouteAgentMessage {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageIntent {
    UserWithToolCalls { tool_calls: Vec<ToolCall> },
    UserWithoutToolCalls,
    DispatchToolCalls,
    ResolveToolCall,
    CompleteTurn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMessage {
    pub id: String,
    pub agent: Entity,
    pub message: Message,
    pub intent: MessageIntent,
}

impl Event for AgentMessage {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentFailureKind {
    Agent,
    Inference,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentFailure {
    pub id: String,
    pub agent: Entity,
    pub kind: AgentFailureKind,
    pub message: String,
}

impl Event for AgentFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentContextMessagesUpdated {
    pub agent: Entity,
    pub messages: Vec<Message>,
}

impl Event for AgentContextMessagesUpdated {}

pub type MessageResource = ResourceRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentResourcesUsed {
    pub id: String,
    pub agent: Entity,
    pub resources: Vec<MessageResource>,
}

impl Event for AgentResourcesUsed {}

fn validate_part(part: &str) -> Result<(), ResourceNameError> {
    if part.is_empty() || part == "." || part == ".." {
        return Err(ResourceNameError::InvalidName);
    }
    if part
        .chars()
        .any(|character| character.is_control() || character == '\\')
    {
        return Err(ResourceNameError::InvalidCharacter);
    }
    Ok(())
}

fn validate_tag(tag: &str) -> Result<(), AgentImageReferenceError> {
    if tag.is_empty() || tag.len() > 128 {
        return Err(AgentImageReferenceError::InvalidTag);
    }
    let mut characters = tag.chars();
    let first = characters
        .next()
        .ok_or(AgentImageReferenceError::InvalidTag)?;
    if first == '.' || first == '-' || !is_tag_character(first) {
        return Err(AgentImageReferenceError::InvalidTag);
    }
    if !characters.all(is_tag_character) {
        return Err(AgentImageReferenceError::InvalidTag);
    }
    Ok(())
}

fn is_tag_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_names_are_split_into_scope_and_name() {
        let name = ResourceName::new("local/code-review").unwrap();

        assert_eq!(name.scope(), "local");
        assert_eq!(name.name(), "code-review");
        assert_eq!(name.to_string(), "local/code-review");
    }

    #[test]
    fn resource_names_reject_path_traversal_and_extra_segments() {
        assert!(ResourceName::new("../review").is_err());
        assert!(ResourceName::new("local/../review").is_err());
        assert!(ResourceName::new("local/review/extra").is_err());
    }

    #[test]
    fn resource_names_report_invalid_characters() {
        assert_eq!(
            ResourceName::new("local/bad\\name"),
            Err(ResourceNameError::InvalidCharacter)
        );
    }

    #[test]
    fn resource_references_validate_provider_ids() {
        let name = ResourceName::new("local/review").unwrap();
        let resource = ResourceRef::new("workflow", name.clone()).unwrap();

        assert_eq!(resource.provider(), "workflow");
        assert_eq!(resource.name(), &name);
        assert_eq!(
            ResourceRef::new("", name.clone()),
            Err(ResourceRefError::EmptyProvider)
        );
        assert_eq!(
            ResourceRef::new("Invalid Provider", name),
            Err(ResourceRefError::InvalidProvider)
        );
    }

    #[test]
    fn agent_image_references_default_to_latest() {
        let reference = AgentImageReference::new("local/coder").unwrap();

        assert_eq!(reference.scope(), "local");
        assert_eq!(reference.name(), "coder");
        assert_eq!(reference.tag(), "latest");
        assert_eq!(reference.to_string(), "local/coder:latest");
    }

    #[test]
    fn agent_image_references_preserve_explicit_tags() {
        let reference = AgentImageReference::new("local/coder:v1.2-rc_1").unwrap();

        assert_eq!(
            reference.resource(),
            &ResourceName::new("local/coder").unwrap()
        );
        assert_eq!(reference.tag(), "v1.2-rc_1");
    }

    #[test]
    fn agent_image_references_reject_invalid_names_and_tags() {
        assert_eq!(
            AgentImageReference::new("coder:latest"),
            Err(AgentImageReferenceError::InvalidName)
        );
        assert_eq!(
            AgentImageReference::new("local/coder:-latest"),
            Err(AgentImageReferenceError::InvalidTag)
        );
        assert_eq!(
            AgentImageReference::new("local/coder:tag:extra"),
            Err(AgentImageReferenceError::InvalidTag)
        );
    }

    #[test]
    fn user_message_intents_distinguish_preselected_tool_calls() {
        let tool_call = ToolCall {
            id: "call-1".into(),
            name: "skill__local__review".into(),
            arguments: "{}".into(),
        };

        assert_eq!(
            MessageIntent::UserWithToolCalls {
                tool_calls: vec![tool_call.clone()],
            },
            MessageIntent::UserWithToolCalls {
                tool_calls: vec![tool_call],
            }
        );
        assert_ne!(
            MessageIntent::UserWithToolCalls {
                tool_calls: Vec::new(),
            },
            MessageIntent::UserWithoutToolCalls
        );
    }

    #[test]
    fn messages_round_trip_through_json() {
        let message = Message::Assistant {
            content: Some("Using the selected workflow.".into()),
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                name: "workflow__local__review".into(),
                arguments: r#"{"path":"src/lib.rs"}"#.into(),
            }],
        };

        let encoded = serde_json::to_string(&message).unwrap();
        let decoded = serde_json::from_str::<Message>(&encoded).unwrap();

        assert_eq!(decoded, message);
    }

    #[test]
    fn shared_agent_message_types_are_events() {
        fn assert_event<EventType: Event>() {}

        assert_event::<AgentMessage>();
        assert_event::<AgentContextMessagesUpdated>();
        assert_event::<AgentResourcesUsed>();
    }
}
