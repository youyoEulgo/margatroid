use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use core_plugin::{Entity, Event};
use serde::{Deserialize, Serialize};

pub use resource_id_plugin::{ResourceId, ResourceIdError};

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
pub struct RouteAgentAssistant {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub agent: Option<ResourceId>,
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<RouteAgentAssistantToolCall>,
}

impl Event for RouteAgentAssistant {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteAgentAssistantToolCall {
    pub id: String,
    pub resource_id: ResourceId,
    pub arguments: String,
}

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

#[derive(Clone, Debug)]
pub struct RouteMclCommand {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub agent: Option<ResourceId>,
    pub command: String,
    pub binding: Option<serde_json::Value>,
    pub reply: std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
}

impl Event for RouteMclCommand {}

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

#[derive(Clone, Debug)]
pub struct InferenceRequestEvent {
    pub id: String,
    pub agent: Entity,
    pub agent_id: ResourceId,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
}
impl Event for InferenceRequestEvent {}

#[derive(Clone, Debug)]
pub struct CapturedInferenceRequest {
    pub id: String,
    pub agent: Entity,
    pub agent_id: ResourceId,
    pub messages: Vec<Message>,
}

impl Event for CapturedInferenceRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedInferenceResponse {
    pub id: String,
    pub agent: Entity,
    pub result: Result<String, String>,
}

impl Event for CapturedInferenceResponse {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallEvent {
    pub turn_id: String,
    pub agent: Entity,
    pub call: ToolCall,
}
impl Event for ToolCallEvent {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentFailureKind {
    Agent,
    Inference,
    Tool,
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

/// Requests a complete replacement of the persisted MCL realtime-context
/// snapshot. The source block is selected explicitly by the Base Driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRealtimeContextWriteRequested {
    pub agent: Entity,
    pub messages: Vec<MclMessage>,
}

impl Event for AgentRealtimeContextWriteRequested {}

/// A synchronous MCL effect asks MemoryPlugin to return the persisted
/// realtime snapshot. The reply stays at the MCL boundary; it is never an
/// implicit Agent creation input.
#[derive(Clone, Debug)]
pub struct AgentRealtimeContextReadRequested {
    pub id: String,
    pub agent: Entity,
}

impl Event for AgentRealtimeContextReadRequested {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRealtimeContextReadCompleted {
    pub id: String,
    pub agent: Entity,
    pub result: Result<Vec<MclMessage>, String>,
}

impl Event for AgentRealtimeContextReadCompleted {}

// The following data types are deliberately free of plugin-specific behavior.
// Domain plugins store and mutate them through the narrow methods below.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LuaVmId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MclMessage {
    pub message: Message,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLuaMessageEnvelope {
    pub turn_id: String,
    pub message: MclMessage,
}

impl MclMessage {
    pub fn new(message: Message, usage: Option<TokenUsage>) -> Self {
        Self { message, usage }
    }
    pub fn message(&self) -> &Message {
        &self.message
    }
    pub fn usage(&self) -> Option<&TokenUsage> {
        self.usage.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockInner {
    Message(Vec<MclMessage>),
    ToolCall(Vec<ToolCall>),
    ResourceId(Vec<ResourceId>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InnerType {
    Message,
    ToolCall,
    ResourceId,
}

impl BlockInner {
    pub fn inner_type(&self) -> InnerType {
        match self {
            Self::Message(_) => InnerType::Message,
            Self::ToolCall(_) => InnerType::ToolCall,
            Self::ResourceId(_) => InnerType::ResourceId,
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Message(v) => v.len(),
            Self::ToolCall(v) => v.len(),
            Self::ResourceId(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockPath {
    pub block_id: String,
    pub inner_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Block {
    pub inners: HashMap<String, BlockInner>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BlockAssembly {
    pub blocks: HashMap<String, Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefMerge {
    Message(Vec<BlockPath>),
    ToolCall(Vec<BlockPath>),
    ResourceId(Vec<BlockPath>),
}

impl RefMerge {
    pub fn paths(&self) -> &[BlockPath] {
        match self {
            Self::Message(v) | Self::ToolCall(v) | Self::ResourceId(v) => v,
        }
    }
    pub fn inner_type(&self) -> InnerType {
        match self {
            Self::Message(_) => InnerType::Message,
            Self::ToolCall(_) => InnerType::ToolCall,
            Self::ResourceId(_) => InnerType::ResourceId,
        }
    }
    pub fn iter(&self, blocks: &BlockAssembly) -> Result<BlockInner, AgentError> {
        let mut out = match self {
            Self::Message(_) => BlockInner::Message(Vec::new()),
            Self::ToolCall(_) => BlockInner::ToolCall(Vec::new()),
            Self::ResourceId(_) => BlockInner::ResourceId(Vec::new()),
        };
        for path in self.paths() {
            let block = blocks
                .blocks
                .get(&path.block_id)
                .ok_or_else(|| AgentError::new(AgentErrorKind::BlockMissing, "block is missing"))?;
            let value = block
                .inners
                .get(&path.inner_id)
                .ok_or_else(|| AgentError::new(AgentErrorKind::InnerMissing, "inner is missing"))?;
            if value.inner_type() != self.inner_type() {
                return Err(AgentError::new(
                    AgentErrorKind::TypeMismatch,
                    "inner type mismatch",
                ));
            }
            match (&mut out, value) {
                (BlockInner::Message(dst), BlockInner::Message(src)) => {
                    dst.extend(src.iter().cloned())
                }
                (BlockInner::ToolCall(dst), BlockInner::ToolCall(src)) => {
                    dst.extend(src.iter().cloned())
                }
                (BlockInner::ResourceId(dst), BlockInner::ResourceId(src)) => {
                    dst.extend(src.iter().cloned())
                }
                _ => {
                    return Err(AgentError::new(
                        AgentErrorKind::TypeMismatch,
                        "inner type mismatch",
                    ))
                }
            }
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RefBlock {
    pub merges: HashMap<String, RefMerge>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RefBlockAssembly {
    pub blocks: HashMap<String, RefBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclDeleteSelection {
    All,
    First,
    Indices(Vec<usize>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MclRealtimeSource {
    pub ref_block_id: String,
    pub message_merge_id: String,
    pub dependencies: Vec<BlockPath>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentErrorKind {
    InvalidRequest,
    AgentMissing,
    DuplicateAgent,
    ResourceMissing,
    BlockMissing,
    InnerMissing,
    TypeMismatch,
    LuaRuntime,
    Mcl,
    Import,
    Inference,
    Tool,
    Memory,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentError {
    pub kind: AgentErrorKind,
    pub message: String,
}

impl AgentError {
    pub fn new(kind: AgentErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > 512 {
            message.truncate(509);
            message.push_str("...");
        }
        Self { kind, message }
    }
}
impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for AgentError {}

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
