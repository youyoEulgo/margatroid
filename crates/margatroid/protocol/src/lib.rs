mod bundle;
mod error;
mod id;
mod request;
mod resource;
mod version;
mod workspace;

pub use bundle::{BundledResource, ResourceManifest, ResourceManifestEntry, WorkspaceBundle};
pub use error::{ApiError, ErrorCode, ErrorResponse};
pub use id::{AgentId, IdentifierError, ProjectName, RequestId, ResourceId, TaskId, WorkspaceId};
pub use request::{
    ExecutionStatus, GetRequestResponse, RequestSummary, SubmitPromptRequest, SubmitPromptResponse,
    TaskResult, TaskSummary,
};
pub use resource::{ContentDigest, InvalidDigest, ResourceKind, ResourceReference};
pub use version::{API_VERSION, CURRENT_SCHEMA_VERSION, SchemaVersion};
pub use workspace::{
    CreateWorkspaceRequest, CreateWorkspaceResponse, ListWorkspacesResponse, WorkspaceAgentSpec,
    WorkspaceSpec, WorkspaceStatus, WorkspaceSummary,
};
