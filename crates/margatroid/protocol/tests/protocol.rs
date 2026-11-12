use margatroid_protocol::*;
use serde_json::{Value, json};

fn parse(kind: &str, message: Value) -> Result<ClientMessage, ProtocolError> {
    parse_client_message(&json!({"type":kind,"id":"request-1","message":message}).to_string())
}
fn target() -> Value {
    json!({"address":"agent:demo/coder:latest"})
}
fn error() -> Value {
    json!({"code":"Rejected","message":"safe failure"})
}

#[test]
fn all_seventeen_client_types_round_trip_and_keep_request_ids() {
    let cases = vec![
        (
            "connection.register",
            json!({"client_type":"webui","name":"browser"}),
        ),
        ("workspace.req", json!({})),
        ("workspace.reload", json!({})),
        ("workspace.start", json!({"path":"/project/workspace.yaml"})),
        ("workspace.stop", json!({"path":"/project/workspace.yaml"})),
        (
            "workspace.restart",
            json!({"path":"/project/workspace.yaml"}),
        ),
        ("agent.req", json!({})),
        (
            "agent.start",
            json!({"workspace":"workspace:local/demo:latest","name":"coder"}),
        ),
        ("agent.stop", target()),
        ("agent.restart", target()),
        ("agent.turn.req", target()),
        ("agent.task.req", target()),
        (
            "agent.turn.abort",
            json!({"address":"agent:demo/coder:latest","turn":"turn-1"}),
        ),
        ("agent.history.req", target()),
        ("agent.log.req", target()),
        ("agent.mcl.req", target()),
        (
            "mcl.execute",
            json!({"address":"agent:demo/coder:latest","command":"CONFIG GET"}),
        ),
    ];
    assert_eq!(cases.len(), 17);
    for (kind, payload) in cases {
        let request = parse(kind, payload).unwrap();
        assert_eq!(request.id(), "request-1");
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["type"], kind);
        assert_eq!(parse_client_message(&encoded.to_string()).unwrap(), request);
    }
}

#[test]
fn retired_types_fields_and_invalid_envelopes_are_not_accepted() {
    for kind in [
        "state.query",
        "workspace.definitions.query",
        "history.query",
        "mcl.command",
    ] {
        assert!(parse(kind, json!({})).is_err());
    }
    for raw in [
        json!({"type":"workspace.req","id":"x"}),
        json!({"type":"workspace.req","id":"x","message":null}),
        json!({"type":"workspace.req","id":"","message":{}}),
        json!({"type":"workspace.req","id":"x\ny","message":{}}),
        json!({"type":"workspace.req","id":"x","message":{},"extra":"PRIVATE"}),
        json!({"type":"workspace.req","id":"x","message":{"extra":"PRIVATE"}}),
    ] {
        let failure = parse_client_message(&raw.to_string()).unwrap_err();
        assert!(!failure.to_string().contains("PRIVATE"));
    }
    assert!(
        parse(
            "workspace.stop",
            json!({"workspace":"workspace:local/demo"})
        )
        .is_err()
    );
    assert!(parse("agent.start",json!({"workspace":"workspace:local/demo","name":"coder","parameters":{"driver":"driver:local/coder","home_root":"/tmp"}})).is_err());
    for name in ["", ".", "..", "../x", "a\\b", "a:b"] {
        assert!(
            parse(
                "agent.start",
                json!({"workspace":"workspace:local/demo","name":name})
            )
            .is_err()
        );
    }
    assert!(parse("agent.stop", json!({"address":"tool:demo/coder"})).is_err());
    assert!(
        parse(
            "agent.turn.abort",
            json!({"address":"agent:demo/coder","turn":""})
        )
        .is_err()
    );
}

#[test]
fn record_queries_use_exclusive_bounds_and_positive_limits() {
    for kind in ["agent.history.req", "agent.log.req", "agent.mcl.req"] {
        let mut request = target();
        request["after_sequence"] = json!(10);
        request["before_sequence"] = json!(21);
        request["limit"] = json!(50);
        assert!(parse(kind, request.clone()).is_ok());
        for patch in [
            json!({"after_sequence":21}),
            json!({"after_sequence":22}),
            json!({"limit":0}),
            json!({"limit":-1}),
            json!({"before_sequence":u64::MAX}),
        ] {
            let mut invalid = request.clone();
            invalid
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(parse(kind, invalid).is_err());
        }
    }
}

#[test]
fn all_twenty_five_server_types_round_trip_and_distinguish_sync_ids() {
    let mut cases = Vec::new();
    for kind in [
        "workspace.start.result",
        "workspace.stop.result",
        "workspace.restart.result",
        "workspace.reload.result",
        "agent.start.result",
        "agent.stop.result",
        "agent.restart.result",
        "agent.turn.abort.result",
        "mcl.execute.result",
    ] {
        cases.push(json!({"type":kind,"id":"request-1","success":true}));
    }
    cases.extend([
        json!({"type":"connection.register.result","id":"request-1","success":true,"client":{"address":"client:webui/browser:1","client_type":"webui"}}),
        json!({"type":"workspace.res","id":"request-1","success":true,"workspaces":[]}),
        json!({"type":"workspace.sync","workspaces":[]}),
        json!({"type":"agent.res","id":"request-1","success":true,"agents":[]}),
        json!({"type":"agent.sync","agents":[]}),
        json!({"type":"agent.turn.res","id":"request-1","success":true,"state":{"address":"agent:demo/coder","turn":null}}),
        json!({"type":"agent.turn.sync","address":"agent:demo/coder","turn":null}),
        json!({"type":"agent.task.res","id":"request-1","success":true,"address":"agent:demo/coder","tasks":[]}),
        json!({"type":"agent.task.sync","address":"agent:demo/coder","tasks":[]}),
        json!({"type":"agent.message.chunk.sync","address":"agent:demo/coder","turn":"turn-1","generation":"gen-1","sequence":1,"delta":{"reasoning":"think"}}),
        json!({"type":"agent.message.full.sync","address":"agent:demo/coder","turn":null,"generation":null,"history_sequence":null,"message":{"type":"user","content":"hello","data":[]},"error":null}),
    ]);
    for kind in ["agent.history.res", "agent.log.res", "agent.mcl.res"] {
        cases.push(json!({"type":kind,"id":"request-1","success":true,"address":"agent:demo/coder","records":[],"next_before_sequence":null,"next_after_sequence":null}));
    }
    cases.push(json!({"type":"agent.log.sync","address":"agent:demo/coder","records":[{"sequence":1,"created_at_ms":0,"level":"info","source":"driver","stage":"execute","message":"ready","details_json":null,"context_json":null,"origin":null,"raw_error":null,"traceback":null}]}));
    cases.push(json!({"type":"agent.mcl.sync","address":"agent:demo/coder","records":[{"sequence":2,"created_at_ms":0,"source":"client:webui/browser:1","turn":null,"content":"CONFIG GET","payload":{"text":"CONFIG GET"}}]}));
    assert_eq!(cases.len(), 25);
    for raw in cases {
        let message: ServerMessage =
            serde_json::from_value(raw.clone()).unwrap_or_else(|e| panic!("{}: {e}", raw["type"]));
        assert_eq!(message.id(), raw.get("id").and_then(Value::as_str));
        let encoded: Value =
            serde_json::from_str(&encode_server_message(&message).unwrap()).unwrap();
        assert_eq!(encoded["type"], raw["type"]);
        assert_eq!(
            serde_json::from_value::<ServerMessage>(encoded).unwrap(),
            message
        );
    }
}

#[test]
fn result_success_error_and_page_cursor_presence_are_strict() {
    for kind in [
        "workspace.start.result",
        "agent.stop.result",
        "agent.turn.abort.result",
    ] {
        for raw in [
            json!({"type":kind,"id":"r","success":false}),
            json!({"type":kind,"id":"r","success":true,"error":error()}),
            json!({"type":kind,"id":"r","success":true,"workspace":{}}),
        ] {
            assert!(serde_json::from_value::<ServerMessage>(raw).is_err());
        }
        let failed: ServerMessage =
            serde_json::from_value(json!({"type":kind,"id":"r","success":false,"error":error()}))
                .unwrap();
        assert!(serde_json::to_value(failed).unwrap().get("data").is_none());
    }
    let result = ServerMessage::AgentHistoryRes {
        id: "r".into(),
        success: true,
        address: "agent:demo/coder".into(),
        records: Some(vec![]),
        next_before_sequence: None,
        next_after_sequence: None,
        error: None,
    };
    let raw = serde_json::to_value(result).unwrap();
    assert!(raw.get("next_before_sequence").unwrap().is_null());
    assert!(raw.get("next_after_sequence").unwrap().is_null());
    for key in ["next_before_sequence", "next_after_sequence"] {
        let mut bad = raw.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<ServerMessage>(bad).is_err());
    }
    let failed = ServerMessage::AgentHistoryRes {
        id: "r".into(),
        success: false,
        address: "agent:demo/coder".into(),
        records: None,
        next_before_sequence: None,
        next_after_sequence: None,
        error: Some(ProtocolErrorDto::new("Failed", "no")),
    };
    let raw = serde_json::to_value(failed).unwrap();
    assert!(raw.get("records").is_none());
    assert!(raw.get("next_before_sequence").is_none());
    let impossible = ServerMessage::WorkspaceStartResult {
        id: "r".into(),
        success: true,
        error: Some(ProtocolErrorDto::new("Failed", "no")),
    };
    assert!(encode_server_message(&impossible).is_err());
}

#[test]
fn complete_workspace_agent_tool_and_mcl_business_data_are_preserved() {
    let workspace = json!({"type":"workspace.sync","workspaces":[{"path":"/project/workspace.yaml","definition":{"name":"demo","project_root":"/project","agents":[{"name":"coder","driver":"driver:local/coder","home_root":"/agent"}]},"status":"inactive","address":null,"error":null}]});
    let message: ServerMessage = serde_json::from_value(workspace.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), workspace);
    let agent = json!({"type":"agent.sync","agents":[{"address":"agent:demo/coder","workspace":"workspace:local/demo","status":"ready","tools":[{"alias":"read","path":"/tools/read","description":"Read","input_schema":{"type":"object"},"owner_task_id":null,"binding":{"opaque":true}}],"error":null}]});
    let message: ServerMessage = serde_json::from_value(agent.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), agent);
    let result = json!({"type":"mcl.execute.result","id":"r","success":true,"data":{"kind":"block_snapshots","value":{"timeline":{},"snapshots":[]}}});
    let message: ServerMessage = serde_json::from_value(result.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), result);
    let mut wrong = result;
    wrong["data"]["kind"] = json!("unknown");
    assert!(serde_json::from_value::<ServerMessage>(wrong).is_err());
}

#[test]
fn message_sync_uses_one_reasoning_field_and_real_history_identity() {
    let full = json!({"type":"agent.message.full.sync","address":"agent:demo/coder","turn":"t","generation":"g","history_sequence":42,"message":{"type":"assistant","sequence":42,"content":"answer","reasoning":"thought","tool_calls":[]},"error":null});
    let parsed: ServerMessage = serde_json::from_value(full.clone()).unwrap();
    assert!(parsed.id().is_none());
    for (key, value) in [
        ("replay", json!({})),
        ("images", json!([])),
        ("api_format", json!("messages")),
        ("sequence", json!(0)),
    ] {
        let mut bad = full.clone();
        bad["message"][key] = value;
        assert!(serde_json::from_value::<ServerMessage>(bad).is_err());
    }
    let mut bad = full;
    bad["history_sequence"] = json!(43);
    assert!(serde_json::from_value::<ServerMessage>(bad).is_err());
    for kind in [
        "state.sync",
        "log",
        "agent.message.finished",
        "agent.message.delta",
    ] {
        assert!(serde_json::from_value::<ServerMessage>(json!({"type":kind})).is_err());
    }
}

#[test]
fn budgets_and_media_validation_do_not_echo_payloads() {
    assert_eq!(
        parse_client_message(&" ".repeat(MAX_MESSAGE_BYTES + 1))
            .unwrap_err()
            .0
            .code,
        "MessageTooLarge"
    );
    let message = |image| json!({"address":"agent:demo/coder","command":"INJECT ? TO entry.pending[-1]","arguments":[[{"type":"user","content":"image","data":[image]}]]});
    assert!(
        parse(
            "mcl.execute",
            message(json!({"media_type":"image/png","data":"iVBORw0KGgo="}))
        )
        .is_ok()
    );
    let secret = "PRIVATE-NOT-BASE64";
    assert!(
        !parse(
            "mcl.execute",
            message(json!({"media_type":"image/png","data":secret}))
        )
        .unwrap_err()
        .to_string()
        .contains(secret)
    );
    let args = json!(["path",0.8,true,[1,2],{"$__toml_private_datetime":"1979-05-27T07:32:00Z"}]);
    let parsed = parse(
        "mcl.execute",
        json!({"address":"agent:demo/coder","command":"CONFIG SET ? ?","arguments":args}),
    )
    .unwrap();
    let ClientMessage::MclExecute { message, .. } = parsed else {
        panic!()
    };
    assert_eq!(message.arguments, args.as_array().unwrap().clone());
    let error = ProtocolErrorDto::new("Invalid", "界".repeat(300));
    assert_eq!(error.message.chars().count(), 170);
    let value = serde_json::to_value(error).unwrap();
    assert_eq!(value["code"], "Invalid");
    assert!(value.get("kind").is_none());
    assert!(value.get("details").is_none());
}

#[test]
fn required_nullable_fields_and_type_constants_are_not_optional() {
    let base = json!({"type":"agent.message.full.sync","address":"agent:demo/coder","turn":null,"generation":null,"history_sequence":null,"message":{"type":"error","message":"safe"},"error":null});
    for key in ["turn", "generation", "history_sequence", "error"] {
        let mut bad = base.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<ServerMessage>(bad).is_err(),
            "{key}"
        );
    }
    let valid = json!({"type":"workspace.sync","workspaces":[{"path":"/work/workspace.yaml","definition":{"name":"w","project_root":"/work","agents":[]},"status":"inactive","address":null,"error":null}]});
    for key in ["address", "error"] {
        let mut bad = valid.clone();
        bad["workspaces"][0].as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<ServerMessage>(bad).is_err());
    }
    let mut bad = base;
    bad["message"]["type"] = json!("assistant_compat");
    assert!(serde_json::from_value::<ServerMessage>(bad).is_err());
    assert!(
        serde_json::from_value::<ProtocolErrorDto>(
            json!({"kind":"Old","code":"New","message":"safe"})
        )
        .is_err()
    );
}

#[test]
fn public_targets_reject_internal_vm_identity_fields() {
    for key in ["instance", "agent_instance", "start_id", "vm_id"] {
        let mut value = target();
        value[key] = json!("private-vm");
        assert!(parse("agent.stop", value).is_err());
        let mut value = json!({"type":"agent.turn.sync","address":"agent:demo/coder","turn":null});
        value[key] = json!("private-vm");
        assert!(serde_json::from_value::<ServerMessage>(value).is_err());
    }
}
