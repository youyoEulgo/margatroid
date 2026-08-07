use std::fmt;
use std::path::PathBuf;

use margatroid_types::{
    AgentImageReference, Message, ResourceName, ResourceRef, WorkspaceAgentDefinition,
    WorkspaceDefinition,
};
use serde::{Deserialize, Serialize};

const MAX_ERROR_MESSAGE_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientRequest {
    #[serde(rename = "workspace.start")]
    WorkspaceStart {
        id: String,
        definition: WorkspaceDefinitionDto,
    },
    #[serde(rename = "agent.message")]
    AgentMessage {
        id: String,
        workspace: WorkspaceRefDto,
        agent: Option<String>,
        content: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "log")]
    Log { record: LogRecordDto },
    #[serde(rename = "workspace.started")]
    WorkspaceStarted {
        id: String,
        workspace: WorkspaceInfoDto,
    },
    #[serde(rename = "agent.message")]
    AgentMessage { message: AgentMessageDto },
    #[serde(rename = "agent.failure")]
    AgentFailure { failure: AgentFailureDto },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecordDto {
    pub timestamp_millis: u64,
    pub level: String,
    pub target: String,
    pub message: String,
    #[serde(default)]
    pub fields: Vec<LogFieldDto>,
    #[serde(default)]
    pub spans: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFieldDto {
    pub name: String,
    pub value: String,
}

impl ClientRequest {
    pub fn start_workspace(id: impl Into<String>, definition: &WorkspaceDefinition) -> Self {
        Self::WorkspaceStart {
            id: id.into(),
            definition: WorkspaceDefinitionDto::from_definition(definition),
        }
    }

    pub fn agent_message(
        id: impl Into<String>,
        workspace: &WorkspaceRefDto,
        agent: Option<String>,
        content: impl Into<String>,
    ) -> Self {
        Self::AgentMessage {
            id: id.into(),
            workspace: workspace.clone(),
            agent,
            content: content.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRefDto {
    pub name: String,
    pub project_root: String,
}

impl WorkspaceRefDto {
    pub fn new(name: impl Into<String>, project_root: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            project_root: project_root.into(),
        }
    }

    pub fn from_definition(definition: &WorkspaceDefinition) -> Self {
        Self::new(
            definition.name.clone(),
            definition.project_root.to_string_lossy().into_owned(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfoDto {
    pub name: String,
    pub project_root: String,
    pub manager: String,
    pub agents: Vec<String>,
}

impl WorkspaceInfoDto {
    pub fn from_definition(definition: &WorkspaceDefinition) -> Self {
        Self {
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into_owned(),
            manager: definition.manager.clone(),
            agents: definition
                .agents
                .iter()
                .map(|agent| agent.name.clone())
                .collect(),
        }
    }

    pub fn reference(&self) -> WorkspaceRefDto {
        WorkspaceRefDto::new(self.name.clone(), self.project_root.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessageDto {
    pub id: String,
    pub workspace: WorkspaceRefDto,
    pub agent: String,
    pub message: Message,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFailureDto {
    pub id: String,
    pub workspace: WorkspaceRefDto,
    pub agent: String,
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDefinitionDto {
    pub name: String,
    pub project_root: String,
    pub manager: String,
    pub agents: Vec<WorkspaceAgentDefinitionDto>,
}

impl WorkspaceDefinitionDto {
    pub fn from_definition(definition: &WorkspaceDefinition) -> Self {
        Self {
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into_owned(),
            manager: definition.manager.clone(),
            agents: definition
                .agents
                .iter()
                .map(WorkspaceAgentDefinitionDto::from_definition)
                .collect(),
        }
    }

    pub fn into_definition(self) -> Result<WorkspaceDefinition, ProtocolError> {
        let agents = self
            .agents
            .into_iter()
            .map(WorkspaceAgentDefinitionDto::into_definition)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkspaceDefinition {
            name: self.name,
            project_root: PathBuf::from(self.project_root),
            manager: self.manager,
            agents,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAgentDefinitionDto {
    pub name: String,
    pub image: String,
    pub resources: Vec<ResourceRefDto>,
    pub disable_resources: Vec<ResourceRefDto>,
    pub memory_path: Option<String>,
}

impl WorkspaceAgentDefinitionDto {
    fn from_definition(definition: &WorkspaceAgentDefinition) -> Self {
        Self {
            name: definition.name.clone(),
            image: definition.image.to_string(),
            resources: definition
                .resources
                .iter()
                .map(ResourceRefDto::from_resource)
                .collect(),
            disable_resources: definition
                .disable_resources
                .iter()
                .map(ResourceRefDto::from_resource)
                .collect(),
            memory_path: definition
                .memory_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }

    fn into_definition(self) -> Result<WorkspaceAgentDefinition, ProtocolError> {
        let image = AgentImageReference::new(self.image).map_err(|error| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidImageReference,
                format!("invalid AgentImage reference: {error}"),
            )
        })?;
        let resources = self
            .resources
            .into_iter()
            .map(ResourceRefDto::into_resource)
            .collect::<Result<Vec<_>, _>>()?;
        let disable_resources = self
            .disable_resources
            .into_iter()
            .map(ResourceRefDto::into_resource)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkspaceAgentDefinition {
            name: self.name,
            image,
            resources,
            disable_resources,
            memory_path: self.memory_path.map(PathBuf::from),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRefDto {
    pub provider: String,
    pub name: String,
}

impl ResourceRefDto {
    fn from_resource(resource: &ResourceRef) -> Self {
        Self {
            provider: resource.provider().to_owned(),
            name: resource.name().to_string(),
        }
    }

    fn into_resource(self) -> Result<ResourceRef, ProtocolError> {
        let name = ResourceName::new(self.name).map_err(|error| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidResourceReference,
                format!("invalid resource name: {error}"),
            )
        })?;
        ResourceRef::new(self.provider, name).map_err(|error| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidResourceReference,
                format!("invalid resource reference: {error}"),
            )
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolErrorKind {
    InvalidImageReference,
    InvalidResourceReference,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError {
    kind: ProtocolErrorKind,
    message: String,
}

impl ProtocolError {
    fn new(kind: ProtocolErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MAX_ERROR_MESSAGE_BYTES {
            let suffix = "...";
            let mut boundary = MAX_ERROR_MESSAGE_BYTES - suffix.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(suffix);
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> ProtocolErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn definition() -> WorkspaceDefinition {
        WorkspaceDefinition {
            name: "demo".into(),
            project_root: Path::new("/tmp/demo").into(),
            manager: "coder".into(),
            agents: vec![WorkspaceAgentDefinition {
                name: "coder".into(),
                image: AgentImageReference::new("local/coder:v1").unwrap(),
                resources: vec![ResourceRef::new(
                    "skill",
                    ResourceName::new("local/context").unwrap(),
                )
                .unwrap()],
                disable_resources: Vec::new(),
                memory_path: None,
            }],
        }
    }

    #[test]
    fn request_uses_stable_workspace_start_shape() {
        let value =
            serde_json::to_value(ClientRequest::start_workspace("request-1", &definition()))
                .unwrap();
        assert_eq!(value["type"], "workspace.start");
        assert_eq!(value["id"], "request-1");
        assert_eq!(value["definition"]["agents"][0]["image"], "local/coder:v1");
        assert_eq!(
            value["definition"]["agents"][0]["resources"][0]["name"],
            "local/context"
        );
    }

    #[test]
    fn dto_round_trips_to_domain_definition() {
        let original = definition();
        let dto = WorkspaceDefinitionDto::from_definition(&original);
        assert_eq!(dto.into_definition().unwrap(), original);
    }

    #[test]
    fn agent_message_uses_workspace_identity_and_optional_agent_name() {
        let workspace = WorkspaceRefDto::new("demo", "/tmp/demo");
        let value = serde_json::to_value(ClientRequest::agent_message(
            "message-1",
            &workspace,
            Some("reviewer".into()),
            "Review this change.",
        ))
        .unwrap();

        assert_eq!(value["type"], "agent.message");
        assert_eq!(value["workspace"]["name"], "demo");
        assert_eq!(value["workspace"]["project_root"], "/tmp/demo");
        assert_eq!(value["agent"], "reviewer");
        assert_eq!(value["content"], "Review this change.");
    }

    #[test]
    fn omitted_agent_is_encoded_for_manager_fallback() {
        let workspace = WorkspaceRefDto::new("demo", "/tmp/demo");
        let value = serde_json::to_value(ClientRequest::agent_message(
            "message-1",
            &workspace,
            None,
            "Hello.",
        ))
        .unwrap();

        assert!(value["agent"].is_null());
    }

    #[test]
    fn workspace_started_exposes_manager_and_selectable_agents() {
        let event = ServerEvent::WorkspaceStarted {
            id: "request-1".into(),
            workspace: WorkspaceInfoDto::from_definition(&definition()),
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "workspace.started");
        assert_eq!(value["workspace"]["manager"], "coder");
        assert_eq!(value["workspace"]["agents"][0], "coder");
    }

    #[test]
    fn agent_message_event_exposes_resolved_route_and_message() {
        let event = ServerEvent::AgentMessage {
            message: AgentMessageDto {
                id: "message-1".into(),
                workspace: WorkspaceRefDto::new("demo", "/tmp/demo"),
                agent: "coder".into(),
                message: Message::Assistant {
                    content: Some("Done.".into()),
                    tool_calls: Vec::new(),
                },
            },
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "agent.message");
        assert_eq!(value["message"]["agent"], "coder");
        assert_eq!(value["message"]["message"]["Assistant"]["content"], "Done.");
    }

    #[test]
    fn server_log_event_decodes_without_llm_fields() {
        let event: ServerEvent = serde_json::from_str(
            r#"{
                "type": "log",
                "record": {
                    "timestamp_millis": 1,
                    "level": "INFO",
                    "target": "workspace",
                    "message": "started"
                }
            }"#,
        )
        .unwrap();
        match event {
            ServerEvent::Log { record } => {
                assert_eq!(record.message, "started");
                assert!(record.fields.is_empty());
            }
            _ => panic!("expected a log event"),
        }
    }
}
