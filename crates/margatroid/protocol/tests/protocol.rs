use margatroid_protocol::*;
use serde_json::json;

fn parse(value: serde_json::Value) -> Result<ClientMessage, ProtocolError> {
    parse_client_message(&value.to_string())
}
fn workspace() -> serde_json::Value {
    json!({"id": "workspace:local/demo:latest", "name": "demo", "project_root": "/project"})
}

#[test]
fn path_start_and_restart_require_fresh_paths_and_reject_old_catalog_identity() {
    for kind in ["workspace.start", "workspace.restart"] {
        assert!(parse(json!({"type": kind, "id": "request", "message": {"path": "/definitions/demo.yaml"}})).is_ok());
        for message in [
            json!({"workspace": "workspace:local/demo", "expected_revision": "old"}),
            json!({"path": ""}),
            json!({"path": "\u{0000}"}),
        ] {
            assert!(parse(json!({"type": kind, "id": "request", "message": message})).is_err());
        }
    }
}

#[test]
fn explicit_targets_identifiers_and_unknown_fields_are_strict_and_errors_are_safe() {
    let secret = "PRIVATE-CONTENT-DO-NOT-ECHO";
    for kind in ["mcl.command", "history.query", "agent.turn.abort"] {
        for agent in [None, Some(serde_json::Value::Null)] {
            let mut message = json!({"workspace": workspace(), "private": secret});
            if let Some(agent) = agent {
                message["agent"] = agent;
            }
            let error =
                parse(json!({"type": kind, "id": "request", "message": message})).unwrap_err();
            assert!(error.to_string().contains("message.agent"));
            assert!(!error.to_string().contains(secret));
        }
    }
    for value in [
        json!({"type": "state.query", "id": "request"}),
        json!({"type": "state.query", "id": "request", "message": null}),
        json!({"type": "state.query", "id": "", "message": {}}),
        json!({"type": "state.query", "id": "bad\nrequest", "message": {}}),
        json!({"type": "state.query", "id": "request", "extra": secret, "message": {}}),
        json!({"type": "state.query", "id": "request", "message": {"extra": secret}}),
        json!({"type": "history.query", "id": "request", "message": {"workspace": workspace(), "agent": "tool:local/name"}}),
        json!({"type": "agent.turn.abort", "id": "request", "message": {"workspace": workspace(), "agent": "agent:demo/coder", "turn_id": "turn"}}),
    ] {
        assert!(!parse(value).unwrap_err().to_string().contains(secret));
    }
}

#[test]
fn start_parameters_are_explicit_and_member_names_cannot_traverse_paths() {
    let base = json!({"type": "agent.start", "id": "request", "message": {"workspace": "workspace:local/demo", "name": "coder", "parameters": {"driver": "driver:local/coder", "home_root": "agents/coder"}}});
    assert!(parse(base.clone()).is_ok());
    for name in ["", ".", "..", "../escape", "a\\b", "bad\nname"] {
        let mut value = base.clone();
        value["message"]["name"] = json!(name);
        assert!(parse(value).is_err());
    }
    let mut missing = base;
    missing["message"]["parameters"]
        .as_object_mut()
        .unwrap()
        .remove("driver");
    assert!(parse(missing).is_err());
}

#[test]
fn generic_arguments_preserve_scalar_array_and_toml_datetime_bridge_values() {
    let arguments =
        json!(["path.key", 0.8, true, [1,2], {"$__toml_private_datetime": "1979-05-27T07:32:00Z"}]);
    let message = parse(json!({"type": "mcl.command", "id": "request", "message": {"workspace": workspace(), "agent": "agent:demo/coder", "command": "CONFIG SET ? ?", "arguments": arguments}})).unwrap();
    let ClientMessage::MclCommand { message, .. } = message else {
        panic!("command");
    };
    assert_eq!(message.arguments, arguments.as_array().unwrap().clone());
}

#[test]
fn request_size_and_image_encoding_limits_are_enforced_without_payload_echo() {
    assert_eq!(
        parse_client_message(&" ".repeat(MAX_MESSAGE_BYTES + 1))
            .unwrap_err()
            .0
            .kind,
        "MessageTooLarge"
    );
    let image = json!({"media_type": "image/png", "data": "iVBORw0KGgo="});
    let message = |image| json!({"type": "mcl.command", "id": "image", "message": {"workspace": workspace(), "agent": "agent:demo/coder", "command": "INJECT ? TO entry.pending[-1]", "arguments": [[{"type": "user", "content": "image", "data": [image]}]]}});
    assert!(parse(message(image)).is_ok());
    let secret = "PRIVATE-NOT-BASE64";
    let error = parse(message(json!({"media_type": "image/png", "data": secret}))).unwrap_err();
    assert!(!error.to_string().contains(secret));
    assert!(
        parse(message(
            json!({"media_type": "image/png", "data": "aGVsbG8="})
        ))
        .is_err()
    );
}

#[test]
fn stream_and_history_wire_preserve_existing_fields_without_internal_domain_types() {
    let stream = ServerMessage::AgentMessageReasoningDelta {
        id: "model-request".into(),
        agent: "agent:demo/coder".into(),
        agent_instance: "driver-1".into(),
        sequence: 7,
        content: "thinking".into(),
    };
    let value: serde_json::Value =
        serde_json::from_str(&encode_server_message(&stream).unwrap()).unwrap();
    assert_eq!(value["type"], "agent.message.reasoning_delta");
    assert_eq!(value["sequence"], 7);
    assert!(value.get("kind").is_none());
    let reference: WorkspaceReferenceDto = serde_json::from_value(workspace()).unwrap();
    let history = ServerMessage::HistoryResult {
        id: "query".into(),
        history: AgentHistoryDto {
            workspace: reference,
            agent: "agent:demo/coder".into(),
            messages: vec![HistoryMessageDto {
                source: None,
                message_type: Some("user".into()),
                sequence: 73,
                turn: None,
                kind: "message".into(),
                content: "saved".into(),
                payload: json!({"type": "user", "content": "saved"}),
                created_at_ms: 1_700_000_000_123,
            }],
        },
    };
    let value: serde_json::Value =
        serde_json::from_str(&encode_server_message(&history).unwrap()).unwrap();
    assert_eq!(
        value["history"]["workspace"]["id"],
        "workspace:local/demo:latest"
    );
    let record = &value["history"]["messages"][0];
    assert_eq!(record["sequence"], 73);
    assert_eq!(record["type"], "user");
    assert!(record.get("source").is_none());
    assert!(record.get("key").is_none());
    let mcl = HistoryMessageDto {
        sequence: 74,
        turn: None,
        message_type: None,
        source: Some("client:webui/browser:latest".into()),
        kind: "mcl".into(),
        content: "run".into(),
        payload: json!({"text":"run"}),
        created_at_ms: 0,
    };
    let mcl = serde_json::to_value(mcl).unwrap();
    assert_eq!(mcl["source"], "client:webui/browser:latest");
    assert!(mcl.get("type").is_none());
    assert_eq!(
        value["history"]["messages"][0]["created_at_ms"],
        1_700_000_000_123_u64
    );
}

#[test]
fn error_messages_are_bounded_by_utf8_bytes() {
    let error = ProtocolErrorDto::new("InvalidRequest", "界".repeat(300));
    assert!(error.message.len() <= 512);
    assert!(error.message.is_char_boundary(error.message.len()));
    assert_eq!(error.message.chars().count(), 170);
}
