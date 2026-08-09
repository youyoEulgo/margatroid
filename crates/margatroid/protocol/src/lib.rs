use std::fmt;
use std::path::PathBuf;

use margatroid_types::{
    AgentImageReference, Message, ResourceName, ResourceRef, RouteAgentMessage, StartWorkspace,
    WorkspaceAgentDefinition, WorkspaceDefinition, WorkspaceReference,
};
use serde::{Deserialize, Serialize};
use server_plugin::{RegisterConnection, WebSocketConnectionId};

const MAX_ERROR_MESSAGE_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientRequest {
    #[serde(rename = "connection.register")]
    ConnectionRegister {
        id: String,
        message: RegisterConnectionDto,
    },
    #[serde(rename = "workspace.start")]
    WorkspaceStart {
        id: String,
        message: StartWorkspaceDto,
    },
    #[serde(rename = "agent.message")]
    AgentMessage {
        id: String,
        message: RouteAgentMessageDto,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "log")]
    Log { record: LogRecordDto },
    #[serde(rename = "state.sync")]
    StateSync { state: BackendStateDto },
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterConnectionDto {
    pub client_type: String,
}

impl RegisterConnectionDto {
    pub fn into_domain(
        self,
        id: String,
        connection_id: WebSocketConnectionId,
    ) -> RegisterConnection {
        RegisterConnection {
            id,
            connection_id,
            client_type: self.client_type,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartWorkspaceDto {
    pub definition: WorkspaceDefinitionDto,
}

impl StartWorkspaceDto {
    pub fn into_domain(self, id: String) -> Result<StartWorkspace, ProtocolError> {
        Ok(StartWorkspace {
            id,
            definition: self.definition.into_domain()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentMessageDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<String>,
    pub message: Message,
}

impl RouteAgentMessageDto {
    pub fn into_domain(self, id: String) -> RouteAgentMessage {
        RouteAgentMessage {
            id,
            workspace: self.workspace.into_domain(),
            agent: self.agent,
            message: self.message,
        }
    }
}

impl ClientRequest {
    pub fn register_connection(id: impl Into<String>, client_type: impl Into<String>) -> Self {
        Self::ConnectionRegister {
            id: id.into(),
            message: RegisterConnectionDto {
                client_type: client_type.into(),
            },
        }
    }

    pub fn start_workspace(id: impl Into<String>, definition: &WorkspaceDefinition) -> Self {
        Self::WorkspaceStart {
            id: id.into(),
            message: StartWorkspaceDto {
                definition: WorkspaceDefinitionDto::from_domain(definition),
            },
        }
    }

    pub fn agent_message(
        id: impl Into<String>,
        workspace: &WorkspaceReferenceDto,
        agent: Option<String>,
        message: Message,
    ) -> Self {
        Self::AgentMessage {
            id: id.into(),
            message: RouteAgentMessageDto {
                workspace: workspace.clone(),
                agent,
                message,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceReferenceDto {
    pub name: String,
    pub project_root: String,
}

impl WorkspaceReferenceDto {
    pub fn new(name: impl Into<String>, project_root: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            project_root: project_root.into(),
        }
    }

    pub fn from_domain(definition: &WorkspaceDefinition) -> Self {
        Self::new(
            definition.name.clone(),
            definition.project_root.to_string_lossy().into_owned(),
        )
    }

    pub fn into_domain(self) -> WorkspaceReference {
        WorkspaceReference {
            name: self.name,
            project_root: PathBuf::from(self.project_root),
        }
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
    pub fn from_domain(definition: &WorkspaceDefinition) -> Self {
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

    pub fn reference(&self) -> WorkspaceReferenceDto {
        WorkspaceReferenceDto::new(self.name.clone(), self.project_root.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendStateDto {
    pub workspaces: Vec<WorkspaceInfoDto>,
    #[serde(default)]
    pub histories: Vec<AgentHistoryDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHistoryDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: String,
    pub messages: Vec<HistoryMessageDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessageDto {
    pub sequence: i64,
    pub turn_id: String,
    pub message: Message,
    #[serde(default)]
    pub resources: Vec<ResourceRefDto>,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessageDto {
    pub id: String,
    pub workspace: WorkspaceReferenceDto,
    pub agent: String,
    pub message: Message,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFailureDto {
    pub id: String,
    pub workspace: WorkspaceReferenceDto,
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
    pub fn from_domain(definition: &WorkspaceDefinition) -> Self {
        Self {
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into_owned(),
            manager: definition.manager.clone(),
            agents: definition
                .agents
                .iter()
                .map(WorkspaceAgentDefinitionDto::from_domain)
                .collect(),
        }
    }

    pub fn into_domain(self) -> Result<WorkspaceDefinition, ProtocolError> {
        let agents = self
            .agents
            .into_iter()
            .map(WorkspaceAgentDefinitionDto::into_domain)
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
    fn from_domain(definition: &WorkspaceAgentDefinition) -> Self {
        Self {
            name: definition.name.clone(),
            image: definition.image.to_string(),
            resources: definition
                .resources
                .iter()
                .map(ResourceRefDto::from_domain)
                .collect(),
            disable_resources: definition
                .disable_resources
                .iter()
                .map(ResourceRefDto::from_domain)
                .collect(),
            memory_path: definition
                .memory_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }

    fn into_domain(self) -> Result<WorkspaceAgentDefinition, ProtocolError> {
        let image = AgentImageReference::new(self.image).map_err(|error| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidImageReference,
                format!("invalid AgentImage reference: {error}"),
            )
        })?;
        let resources = self
            .resources
            .into_iter()
            .map(ResourceRefDto::into_domain)
            .collect::<Result<Vec<_>, _>>()?;
        let disable_resources = self
            .disable_resources
            .into_iter()
            .map(ResourceRefDto::into_domain)
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
    fn from_domain(resource: &ResourceRef) -> Self {
        Self {
            provider: resource.provider().to_owned(),
            name: resource.name().to_string(),
        }
    }

    fn into_domain(self) -> Result<ResourceRef, ProtocolError> {
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
        assert_eq!(
            value["message"]["definition"]["agents"][0]["image"],
            "local/coder:v1"
        );
        assert_eq!(
            value["message"]["definition"]["agents"][0]["resources"][0]["name"],
            "local/context"
        );
    }

    #[test]
    fn connection_registration_uses_stable_shape() {
        let value = serde_json::to_value(ClientRequest::register_connection("register-1", "webui"))
            .unwrap();

        assert_eq!(value["type"], "connection.register");
        assert_eq!(value["id"], "register-1");
        assert_eq!(value["message"]["client_type"], "webui");
    }

    #[test]
    fn dto_round_trips_to_domain_definition() {
        let original = definition();
        let dto = WorkspaceDefinitionDto::from_domain(&original);
        assert_eq!(dto.into_domain().unwrap(), original);
    }

    #[test]
    fn agent_message_uses_workspace_identity_and_optional_agent_name() {
        let workspace = WorkspaceReferenceDto::new("demo", "/tmp/demo");
        let value = serde_json::to_value(ClientRequest::agent_message(
            "message-1",
            &workspace,
            Some("reviewer".into()),
            Message::User {
                content: "Review this change.".into(),
            },
        ))
        .unwrap();

        assert_eq!(value["type"], "agent.message");
        assert_eq!(value["message"]["workspace"]["name"], "demo");
        assert_eq!(value["message"]["workspace"]["project_root"], "/tmp/demo");
        assert_eq!(value["message"]["agent"], "reviewer");
        assert_eq!(
            value["message"]["message"]["User"]["content"],
            "Review this change."
        );
    }

    #[test]
    fn omitted_agent_is_encoded_for_manager_fallback() {
        let workspace = WorkspaceReferenceDto::new("demo", "/tmp/demo");
        let value = serde_json::to_value(ClientRequest::agent_message(
            "message-1",
            &workspace,
            None,
            Message::User {
                content: "Hello.".into(),
            },
        ))
        .unwrap();

        assert!(value["message"]["agent"].is_null());
    }

    #[test]
    fn workspace_started_exposes_manager_and_selectable_agents() {
        let event = ServerEvent::WorkspaceStarted {
            id: "request-1".into(),
            workspace: WorkspaceInfoDto::from_domain(&definition()),
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "workspace.started");
        assert_eq!(value["workspace"]["manager"], "coder");
        assert_eq!(value["workspace"]["agents"][0], "coder");
    }

    #[test]
    fn state_sync_contains_the_complete_workspace_snapshot() {
        let event = ServerEvent::StateSync {
            state: BackendStateDto {
                workspaces: vec![WorkspaceInfoDto::from_domain(&definition())],
                histories: vec![AgentHistoryDto {
                    workspace: WorkspaceReferenceDto::new("demo", "/tmp/demo"),
                    agent: "coder".into(),
                    messages: vec![HistoryMessageDto {
                        sequence: 1,
                        turn_id: "turn-1".into(),
                        message: Message::User {
                            content: "hello".into(),
                        },
                        resources: vec![ResourceRefDto {
                            provider: "skill".into(),
                            name: "local/context".into(),
                        }],
                        created_at_ms: 42,
                    }],
                }],
            },
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "state.sync");
        assert_eq!(value["state"]["workspaces"][0]["name"], "demo");
        assert_eq!(value["state"]["workspaces"][0]["manager"], "coder");
        assert_eq!(value["state"]["histories"][0]["agent"], "coder");
        assert_eq!(
            value["state"]["histories"][0]["messages"][0]["turn_id"],
            "turn-1"
        );
        assert_eq!(
            value["state"]["histories"][0]["messages"][0]["resources"][0]["provider"],
            "skill"
        );
    }

    #[test]
    fn agent_message_event_exposes_resolved_route_and_message() {
        let event = ServerEvent::AgentMessage {
            message: AgentMessageDto {
                id: "message-1".into(),
                workspace: WorkspaceReferenceDto::new("demo", "/tmp/demo"),
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
