use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use core_plugin::{Entity, Event};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceIdError {
    Empty,
    InvalidType,
    InvalidScope,
    InvalidName,
    InvalidTag,
    InvalidFormat,
}

impl fmt::Display for ResourceIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "resource id cannot be empty",
            Self::InvalidType => "resource type is invalid",
            Self::InvalidScope => "resource scope is invalid",
            Self::InvalidName => "resource name is invalid",
            Self::InvalidTag => "resource tag is invalid",
            Self::InvalidFormat => "resource id format is invalid",
        })
    }
}

impl std::error::Error for ResourceIdError {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId {
    resource_type: String,
    scope: String,
    name: String,
    tag: String,
}

impl ResourceId {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ResourceIdError> {
        value.as_ref().parse()
    }

    pub fn new(
        resource_type: impl Into<String>,
        scope: impl Into<String>,
        name: impl Into<String>,
        tag: Option<impl Into<String>>,
    ) -> Result<Self, ResourceIdError> {
        let resource_type = resource_type.into();
        let scope = scope.into();
        let name = name.into();
        let tag = tag.map(Into::into).unwrap_or_else(|| "latest".into());
        validate_resource_type(&resource_type)?;
        validate_resource_part(&scope, ResourceIdError::InvalidScope)?;
        validate_resource_part(&name, ResourceIdError::InvalidName)?;
        validate_resource_tag(&tag)?;
        Ok(Self {
            resource_type,
            scope,
            name,
            tag,
        })
    }

    pub fn resource_type(&self) -> &str {
        &self.resource_type
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }
}

impl FromStr for ResourceId {
    type Err = ResourceIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(ResourceIdError::Empty);
        }
        let (resource_type, remainder) = value
            .split_once(':')
            .ok_or(ResourceIdError::InvalidFormat)?;
        let (scope, name_and_tag) = remainder
            .split_once('/')
            .ok_or(ResourceIdError::InvalidFormat)?;
        if name_and_tag.contains('/') {
            return Err(ResourceIdError::InvalidFormat);
        }
        let (name, tag) = match name_and_tag.split_once(':') {
            Some((name, tag)) => {
                if tag.contains(':') {
                    return Err(ResourceIdError::InvalidFormat);
                }
                (name, Some(tag.to_owned()))
            }
            None => (name_and_tag, None),
        };
        Self::new(resource_type, scope, name, tag)
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}/{}:{}",
            self.resource_type, self.scope, self.name, self.tag
        )
    }
}

impl Serialize for ResourceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ResourceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

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
    pub id: ResourceId,
    pub image: ResourceId,
    pub resources: Vec<ResourceId>,
    pub disable_resources: Vec<ResourceId>,
    pub memory_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceDefinition {
    pub id: ResourceId,
    pub name: String,
    pub project_root: PathBuf,
    pub manager: String,
    pub agents: Vec<WorkspaceAgentDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceReference {
    pub id: ResourceId,
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
    pub agent: Option<ResourceId>,
    pub message: Message,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteAgentTurnAbort {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub agent: Option<ResourceId>,
}

impl Event for RouteAgentTurnAbort {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentVisibilityRouteAction {
    Inject,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteAgentVisibility {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub agent: Option<ResourceId>,
    pub resource_id: ResourceId,
    pub action: AgentVisibilityRouteAction,
}

impl Event for RouteAgentVisibility {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteAgentWorkflowAttach {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub agent: Option<ResourceId>,
    pub resource_id: ResourceId,
}

impl Event for RouteAgentWorkflowAttach {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteAgentWorkflowDetach {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub agent: Option<ResourceId>,
    pub instance_id: String,
}

impl Event for RouteAgentWorkflowDetach {}

impl Event for RouteAgentMessage {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool_name: String,
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
        #[serde(default)]
        reasoning: Option<String>,
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        resource_id: ResourceId,
        tool_call_id: String,
        content: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMessage {
    pub id: String,
    pub agent: Entity,
    pub message: Message,
    pub usage: Option<TokenUsage>,
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
    pub tool_context: Vec<Message>,
    pub ordered_messages: Vec<Message>,
}

impl Event for AgentContextMessagesUpdated {}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentHistoryMessageWriteRequested {
    pub id: String,
    pub agent: Entity,
    pub message: Message,
    pub tool_schema: Vec<ToolDefinition>,
    pub usage: Option<TokenUsage>,
}

impl Event for AgentHistoryMessageWriteRequested {}

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

fn validate_resource_type(resource_type: &str) -> Result<(), ResourceIdError> {
    if resource_type.is_empty()
        || !resource_type.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(ResourceIdError::InvalidType);
    }
    Ok(())
}

fn validate_resource_part(part: &str, error: ResourceIdError) -> Result<(), ResourceIdError> {
    if part.is_empty()
        || part == "."
        || part == ".."
        || part
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
    {
        return Err(error);
    }
    Ok(())
}

fn validate_resource_tag(tag: &str) -> Result<(), ResourceIdError> {
    if tag.is_empty() || tag.len() > 128 {
        return Err(ResourceIdError::InvalidTag);
    }
    let mut characters = tag.chars();
    let first = characters.next().ok_or(ResourceIdError::InvalidTag)?;
    if first == '.' || first == '-' || !is_tag_character(first) {
        return Err(ResourceIdError::InvalidTag);
    }
    if !characters.all(is_tag_character) {
        return Err(ResourceIdError::InvalidTag);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_ids_default_to_latest_and_serialize_as_strings() {
        let id = ResourceId::parse("agent:demo/coder").unwrap();

        assert_eq!(id.resource_type(), "agent");
        assert_eq!(id.scope(), "demo");
        assert_eq!(id.name(), "coder");
        assert_eq!(id.tag(), "latest");
        assert_eq!(id.to_string(), "agent:demo/coder:latest");
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            r#""agent:demo/coder:latest""#
        );
        assert_eq!(
            serde_json::from_str::<ResourceId>(r#""skill:local/review""#).unwrap(),
            ResourceId::parse("skill:local/review:latest").unwrap()
        );
    }

    #[test]
    fn resource_ids_reject_ambiguous_or_invalid_parts() {
        assert!(ResourceId::parse("agent:demo/coder:clone0").is_ok());
        assert!(ResourceId::parse("Agent:demo/coder").is_err());
        assert!(ResourceId::parse("agent:demo/coder:tag:extra").is_err());
        assert!(ResourceId::parse("agent:../coder").is_err());
        assert!(ResourceId::parse("agent:demo/coder/extra").is_err());
    }

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
    fn messages_round_trip_through_json() {
        let message = Message::Assistant {
            reasoning: None,
            content: Some("Using the selected workflow.".into()),
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                tool_name: "workflow0_review".into(),
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
        assert_event::<AgentHistoryMessageWriteRequested>();
    }
}
