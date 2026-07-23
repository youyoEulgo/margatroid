use margatroid_protocol::{
    AgentId, AgentImageReference, ApiError, BundledResource, ContentDigest,
    CreateWorkspaceResponse, ErrorCode, ErrorResponse, ExecutionStatus, GetRequestResponse,
    RESOURCE_PACKAGE_FORMAT_VERSION, RequestId, ResourceId, ResourceKind, ResourceManifest,
    ResourceManifestEntry, ResourcePackage, ResourcePackageFile, SchemaVersion,
    SubmitPromptResponse, TaskId, TaskResult, TaskSummary, WorkspaceAgentSpec, WorkspaceBundle,
    WorkspaceId, WorkspaceName, WorkspaceSpec, WorkspaceStatus, WorkspaceSummary,
};
use serde_json::json;

fn digest(character: char) -> ContentDigest {
    ContentDigest::try_from(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

#[test]
fn resource_package_json_shape_is_stable() {
    let package = ResourcePackage {
        format_version: RESOURCE_PACKAGE_FORMAT_VERSION,
        files: vec![ResourcePackageFile {
            path: "SKILL.md".into(),
            content_base64: "dGVzdAo=".into(),
        }],
    };
    let bytes = serde_json::to_vec(&package).unwrap();
    assert_eq!(
        bytes,
        br#"{"format_version":1,"files":[{"path":"SKILL.md","content_base64":"dGVzdAo="}]}"#
    );
    assert_eq!(
        serde_json::from_slice::<ResourcePackage>(&bytes).unwrap(),
        package
    );
}

#[test]
fn workspace_bundle_json_shape_is_stable_and_round_trips() {
    let soul_digest = digest('a');
    let bundle = WorkspaceBundle {
        schema_version: SchemaVersion::current(),
        spec: WorkspaceSpec {
            name: WorkspaceName::new("demo").unwrap(),
            description: Some("demo workspace".into()),
            manager: AgentId::new("manager").unwrap(),
            agents: vec![WorkspaceAgentSpec {
                id: AgentId::new("manager").unwrap(),
                image: AgentImageReference::new("eulgo/manager:v1").unwrap(),
                skills: vec![],
                workflows: vec![],
                memory_volume: None,
            }],
        },
        manifest: ResourceManifest {
            entries: vec![ResourceManifestEntry {
                kind: ResourceKind::Agent,
                logical_name: "manager".into(),
                format_version: 1,
                digest: soul_digest.clone(),
                size_bytes: 6,
                media_type: "text/markdown".into(),
            }],
        },
        resources: vec![BundledResource {
            digest: soul_digest,
            content_base64: "IyBTb3Vs".into(),
        }],
    };

    let value = serde_json::to_value(&bundle).unwrap();
    assert_eq!(
        value,
        json!({
            "schema_version": 1,
            "spec": {
                "name": "demo",
                "description": "demo workspace",
                "manager": "manager",
                "agents": [{
                    "id": "manager",
                    "image": "eulgo/manager:v1",
                    "skills": [],
                    "workflows": []
                }]
            },
            "manifest": {
                "entries": [{
                    "kind": "agent",
                    "logical_name": "manager",
                    "format_version": 1,
                    "digest": format!("sha256:{}", "a".repeat(64)),
                    "size_bytes": 6,
                    "media_type": "text/markdown"
                }]
            },
            "resources": [{
                "digest": format!("sha256:{}", "a".repeat(64)),
                "content_base64": "IyBTb3Vs"
            }]
        })
    );

    assert_eq!(
        serde_json::from_value::<WorkspaceBundle>(value).unwrap(),
        bundle
    );
}

#[test]
fn request_status_and_error_codes_use_snake_case() {
    let response = SubmitPromptResponse {
        request: margatroid_protocol::RequestSummary {
            id: RequestId::new("request-1").unwrap(),
            workspace_id: WorkspaceId::new("workspace-1").unwrap(),
            status: ExecutionStatus::Waiting,
            root_task_id: Some(TaskId::new("task-1").unwrap()),
            submitted_at_ms: 100,
            updated_at_ms: 200,
        },
    };
    assert_eq!(
        serde_json::to_value(response).unwrap()["request"]["status"],
        "waiting"
    );

    let error = ErrorResponse {
        error: ApiError::new(ErrorCode::ResourceInUse, "resource is still referenced")
            .with_request_id(RequestId::new("request-1").unwrap()),
    };
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({
            "error": {
                "code": "resource_in_use",
                "message": "resource is still referenced",
                "request_id": "request-1"
            }
        })
    );
    assert_eq!(ErrorCode::ResourceInUse.http_status(), 409);
    assert_eq!(ErrorCode::QueueFull.http_status(), 429);
}

#[test]
fn execution_status_transitions_are_explicit() {
    assert!(ExecutionStatus::Queued.can_transition_to(ExecutionStatus::Running));
    assert!(ExecutionStatus::Running.can_transition_to(ExecutionStatus::Waiting));
    assert!(ExecutionStatus::Waiting.can_transition_to(ExecutionStatus::Running));
    assert!(ExecutionStatus::Running.can_transition_to(ExecutionStatus::Completed));
    assert!(ExecutionStatus::Completed.is_terminal());
    assert!(ExecutionStatus::Failed.is_terminal());
    assert!(ExecutionStatus::Cancelled.is_terminal());

    assert!(!ExecutionStatus::Queued.can_transition_to(ExecutionStatus::Completed));
    assert!(!ExecutionStatus::Completed.can_transition_to(ExecutionStatus::Running));
}

#[test]
fn workspace_and_task_dtos_round_trip() {
    let workspace = WorkspaceSummary {
        id: WorkspaceId::new("workspace-1").unwrap(),
        name: WorkspaceName::new("demo").unwrap(),
        status: WorkspaceStatus::Running,
        agent_count: 2,
        created_at_ms: 100,
        updated_at_ms: 200,
    };
    let workspace_response = CreateWorkspaceResponse {
        workspace: workspace.clone(),
    };
    let workspace_json = serde_json::to_value(&workspace_response).unwrap();
    assert_eq!(workspace_json["workspace"]["status"], "running");
    assert_eq!(
        serde_json::from_value::<CreateWorkspaceResponse>(workspace_json).unwrap(),
        workspace_response
    );

    let request = margatroid_protocol::RequestSummary {
        id: RequestId::new("request-1").unwrap(),
        workspace_id: workspace.id,
        status: ExecutionStatus::Completed,
        root_task_id: Some(TaskId::new("task-1").unwrap()),
        submitted_at_ms: 100,
        updated_at_ms: 300,
    };
    let task = TaskSummary {
        id: TaskId::new("task-1").unwrap(),
        request_id: request.id.clone(),
        agent_id: Some(AgentId::new("manager").unwrap()),
        status: ExecutionStatus::Completed,
        result: Some(TaskResult {
            content: "done".into(),
            artifacts: vec![ResourceId::new("artifact-1").unwrap()],
            completed_at_ms: 300,
        }),
        error: None,
        created_at_ms: 150,
        updated_at_ms: 300,
    };
    let response = GetRequestResponse {
        request,
        tasks: vec![task],
    };
    let response_json = serde_json::to_value(&response).unwrap();
    assert_eq!(response_json["tasks"][0]["result"]["content"], "done");
    assert!(response_json["tasks"][0].get("error").is_none());
    assert_eq!(
        serde_json::from_value::<GetRequestResponse>(response_json).unwrap(),
        response
    );
}
