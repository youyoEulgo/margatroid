use margatroid_protocol::*;
use serde_json::{Value, json};

fn parse(kind: &str, message: Value) -> Result<ClientMessage, ProtocolError> {
    parse_client_message(&json!({"type":kind,"id":"request-1","message":message}).to_string())
}
fn target() -> Value {
    json!({"address":"/demo/coder"})
}
fn error() -> Value {
    json!({"code":"Rejected","message":"safe failure"})
}

#[test]
fn all_nineteen_client_types_round_trip_and_keep_request_ids() {
    let cases = vec![
        (
            "connection.register",
            json!({"client_type":"webui","name":"browser"}),
        ),
        ("model.config.req", json!({})),
        ("model.config.update", json!({"content":"[[models]]"})),
        ("workspace.req", json!({})),
        ("resource.req", json!({})),
        (
            "workspace.create",
            json!({"address":"/demo","alias":"Demo","project_root":"/project"}),
        ),
        ("workspace.delete", json!({"address":"/demo"})),
        (
            "agent.start",
            json!({"mode":"existing","workspace":"/demo","node_name":"coder"}),
        ),
        ("agent.stop", target()),
        (
            "agent.archive",
            json!({"workspace":"/demo","node_name":"coder"}),
        ),
        ("agent.req", json!({})),
        ("agent.ui.req", target()),
        ("agent.turn.req", target()),
        (
            "agent.turn.abort",
            json!({"address":"/demo/coder","turn":"turn-1"}),
        ),
        ("agent.task.req", target()),
        ("agent.history.req", target()),
        ("agent.log.req", target()),
        ("agent.mcl.req", target()),
        (
            "mcl.execute",
            json!({"address":"/demo/coder","command":"CONFIG GET"}),
        ),
    ];
    assert_eq!(cases.len(), 19);
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
        "log.req",
        "log.append",
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
    assert!(parse("workspace.stop", json!({"workspace":"/demo"})).is_err());
    assert!(parse("agent.start",json!({"workspace":"/demo","name":"coder","parameters":{"driver":"driver:margatroid/coder:dev","home_root":"/tmp"}})).is_err());
    for name in ["", ".", "..", "../x", "a\\b", "a:b"] {
        assert!(parse("agent.start", json!({"workspace":"/demo","name":name})).is_err());
    }
    assert!(parse("agent.stop", json!({"address":"tool:demo/coder:dev"})).is_err());
    assert!(
        parse(
            "agent.turn.abort",
            json!({"address":"/demo/coder","turn":""})
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
fn all_twenty_nine_server_types_round_trip_and_distinguish_sync_ids() {
    let mut cases = Vec::new();
    for kind in [
        "agent.start.result",
        "agent.stop.result",
        "agent.archive.result",
        "workspace.create.result",
        "workspace.delete.result",
        "agent.turn.abort.result",
        "mcl.execute.result",
    ] {
        cases.push(json!({"type":kind,"id":"request-1","success":true}));
    }
    cases.extend([
        json!({"type":"model.config.res","id":"request-1","success":true,"content":"models=[]"}),
        json!({"type":"model.config.sync","content":null,"error":null}),
        json!({"type":"model.config.update.result","id":"request-1","success":true}),
        json!({"type":"connection.register.result","id":"request-1","success":true,"client":{"address":"/clients/browser","client_type":"webui"}}),
        json!({"type":"workspace.res","id":"request-1","success":true,"workspaces":[]}),
        json!({"type":"workspace.sync","workspaces":[]}),
        json!({"type":"agent.res","id":"request-1","success":true,"agents":[]}),
        json!({"type":"agent.sync","agents":[]}),
        json!({"type":"agent.ui.res","id":"request-1","address":"/demo/coder","success":true,"ui":null}),
        json!({"type":"agent.ui.sync","address":"/demo/coder","ui":null,"error":null}),
        json!({"type":"agent.turn.res","id":"request-1","success":true,"state":{"address":"/demo/coder","turn":null}}),
        json!({"type":"agent.turn.sync","address":"/demo/coder","turn":null}),
        json!({"type":"agent.task.res","id":"request-1","success":true,"address":"/demo/coder","tasks":[]}),
        json!({"type":"agent.task.sync","address":"/demo/coder","tasks":[]}),
        json!({"type":"agent.message.chunk.sync","address":"/demo/coder","turn":"turn-1","generation":"gen-1","sequence":1,"content":"","reasoning":"think"}),
        json!({"type":"agent.message.full.sync","address":"/demo/coder","turn":null,"generation":null,"history_sequence":null,"message":{"type":"user","content":"hello","data":[]},"error":null}),
    ]);
    for kind in ["agent.history.res", "agent.log.res", "agent.mcl.res"] {
        cases.push(json!({"type":kind,"id":"request-1","success":true,"address":"/demo/coder","records":[],"next_before_sequence":null,"next_after_sequence":null}));
    }
    cases.push(json!({"type":"agent.log.sync","address":"/demo/coder","records":[{"sequence":1,"created_at_ms":0,"level":"info","source":"driver","stage":"execute","message":"ready","details_json":null,"context_json":null,"origin":null,"raw_error":null,"traceback":null}]}));
    cases.push(json!({"type":"agent.mcl.sync","address":"/demo/coder","records":[{"sequence":2,"created_at_ms":0,"source":"/clients/browser","turn":null,"content":"CONFIG GET","payload":{"text":"CONFIG GET"}}]}));
    cases.push(json!({"type":"resource.res","id":"query","success":true,"resources":[]}));
    assert_eq!(cases.len(), 29);
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
    for kind in ["agent.start.result", "agent.turn.abort.result"] {
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
        address: "/demo/coder".into(),
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
        address: "/demo/coder".into(),
        records: None,
        next_before_sequence: None,
        next_after_sequence: None,
        error: Some(ProtocolErrorDto::new("Failed", "no")),
    };
    let raw = serde_json::to_value(failed).unwrap();
    assert!(raw.get("records").is_none());
    assert!(raw.get("next_before_sequence").is_none());
    let impossible = ServerMessage::AgentStartResult {
        id: "r".into(),
        success: true,
        error: Some(ProtocolErrorDto::new("Failed", "no")),
    };
    assert!(encode_server_message(&impossible).is_err());
}

#[test]
fn complete_workspace_agent_tool_and_mcl_business_data_are_preserved() {
    let workspace = workspace_snapshot();
    let message: ServerMessage = serde_json::from_value(workspace.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), workspace);
    let agent = json!({"type":"agent.sync","agents":[{"address":"/demo/coder","workspace":"/demo","creator":{"address":"/demo","task_id":"driver-fixture"},"status":"running","sandboxes":[],"sandbox":null,"model":null,"tools":[{"alias":"read","path":"/tools/read","description":"Read","input_schema":{"type":"object"},"owner_task_id":null,"binding":{"opaque":true}}],"error":null}]});
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
    let full = json!({"type":"agent.message.full.sync","address":"/demo/coder","turn":"t","generation":"g","history_sequence":42,"message":{"type":"assistant","sequence":42,"content":"answer","reasoning":"thought","tool_calls":[]},"error":null});
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
    let message = |image| json!({"address":"/demo/coder","command":"INJECT ? TO entry.pending[-1]","arguments":[[{"type":"user","content":"image","data":[image]}]]});
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
        json!({"address":"/demo/coder","command":"CONFIG SET ? ?","arguments":args}),
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
    let base = json!({"type":"agent.message.full.sync","address":"/demo/coder","turn":null,"generation":null,"history_sequence":null,"message":{"type":"error","message":"safe"},"error":null});
    for key in ["turn", "generation", "history_sequence", "error"] {
        let mut bad = base.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<ServerMessage>(bad).is_err(),
            "{key}"
        );
    }
    let valid = workspace_snapshot();
    for key in ["address", "alias", "project_root", "status", "agents"] {
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
        assert!(parse("agent.turn.req", value).is_err());
        let mut value = json!({"type":"agent.turn.sync","address":"/demo/coder","turn":null});
        value[key] = json!("private-vm");
        assert!(serde_json::from_value::<ServerMessage>(value).is_err());
    }
}

#[test]
fn agent_resources_and_public_model_fields_are_strict_and_nullable() {
    let mut value = json!({"type":"agent.sync","agents":[{"address":"/demo/coder","workspace":"/demo","creator":{"address":"/demo","task_id":"driver-fixture"},"status":"running","tools":[],"sandboxes":[{"alias":"read_only","path":"/sandbox/read-only.toml"}],"sandbox":"read_only","model":{"id":"model-route","model":"model-name","provider":"openai","api_type":"responses","context_window_tokens":131072,"thinking":null,"reasoning_effort":"high"},"error":null}]});
    let message: ServerMessage = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), value);
    for field in ["creator", "sandboxes", "sandbox", "model"] {
        let mut missing = value.clone();
        missing["agents"][0].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ServerMessage>(missing).is_err());
    }
    for field in ["api_key", "base_url", "instance"] {
        let mut internal = value.clone();
        internal["agents"][0]["model"][field] = json!("private");
        assert!(serde_json::from_value::<ServerMessage>(internal).is_err());
    }
    value["agents"][0]["sandbox"] = Value::Null;
    value["agents"][0]["model"] = Value::Null;
    value["agents"][0]["sandboxes"] = json!([]);
    assert!(serde_json::from_value::<ServerMessage>(value).is_ok());
}

#[test]
fn driver_ui_messages_enforce_complete_resource_and_nullable_contracts() {
    let bundle = json!({"revision":format!("sha256:{}","0".repeat(64)),"entry":"index.html","files":[{"path":"index.html","media_type":"text/html; charset=utf-8","data":"aGk="}]});
    let value =
        json!({"type":"agent.ui.res","id":"ui","address":"/demo/coder","success":true,"ui":bundle});
    let message: ServerMessage = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), value);
    let absent = ServerMessage::AgentUiRes {
        id: "missing".into(),
        success: true,
        address: "/demo/coder".into(),
        ui: None,
        error: None,
    };
    assert!(
        serde_json::to_value(absent)
            .unwrap()
            .get("ui")
            .unwrap()
            .is_null()
    );
    for patch in [
        json!({"entry":"other.html"}),
        json!({"revision":"wrong"}),
        json!({"files":[]}),
        json!({"files":[{"path":"../index.html","media_type":"text/html","data":"aGk="}]}),
        json!({"files":[{"path":"index.html","media_type":"text/html","data":"%%%"}]}),
    ] {
        let mut invalid = value.clone();
        invalid["ui"]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(serde_json::from_value::<ServerMessage>(invalid).is_err());
    }
    let mut duplicate = value.clone();
    duplicate["ui"]["files"]
        .as_array_mut()
        .unwrap()
        .push(bundle["files"][0].clone());
    assert!(serde_json::from_value::<ServerMessage>(duplicate).is_err());
    for value in [
        json!({"type":"agent.ui.res","id":"ui","address":"/demo/coder","success":true}),
        json!({"type":"agent.ui.res","id":"ui","address":"/demo/coder","success":false,"ui":null,"error":error()}),
        json!({"type":"agent.ui.sync","address":"/demo/coder","ui":bundle,"error":error()}),
        json!({"type":"agent.ui.sync","address":"/demo/coder","ui":null}),
    ] {
        assert!(serde_json::from_value::<ServerMessage>(value).is_err());
    }
    assert!(
        serde_json::from_value::<ServerMessage>(
            json!({"type":"agent.ui.sync","address":"/demo/coder","ui":null,"error":error()})
        )
        .is_ok()
    );
}

#[test]
fn cumulative_chunk_requires_complete_text_pair_and_rejects_delta() {
    let base = json!({"type":"agent.message.chunk.sync","address":"/demo/coder","turn":"turn-1","generation":"gen-1","sequence":1,"content":"你好","reasoning":"思考"});
    let message: ServerMessage = serde_json::from_value(base.clone()).unwrap();
    assert!(encode_server_message(&message).is_ok());
    for field in ["content", "reasoning"] {
        let mut value = base.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ServerMessage>(value).is_err());
    }
    let mut old = base.clone();
    old["delta"] = json!({"content":"x"});
    assert!(serde_json::from_value::<ServerMessage>(old).is_err());
    let mut empty = base;
    empty["content"] = json!("");
    empty["reasoning"] = json!("");
    assert!(serde_json::from_value::<ServerMessage>(empty).is_err());
}

#[test]
fn returned_turn_and_free_generation_chunk_preserve_explicit_optional_association() {
    let response = ServerMessage::MclExecuteResult {
        id: "turn-request".into(),
        success: true,
        data: Some(MclExecuteDataDto::Turn {
            value: "turn:one".into(),
        }),
        error: None,
    };
    let value: Value = serde_json::from_str(&encode_server_message(&response).unwrap()).unwrap();
    assert_eq!(value["data"], json!({"kind":"turn","value":"turn:one"}));
    let raw = json!({"type":"agent.message.chunk.sync","address":"/demo/coder","turn":null,"generation":"free-request","sequence":1,"content":"summary","reasoning":""});
    let parsed: ServerMessage = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&encode_server_message(&parsed).unwrap()).unwrap(),
        raw
    );
    let mut missing = raw.clone();
    missing.as_object_mut().unwrap().remove("turn");
    assert!(serde_json::from_value::<ServerMessage>(missing).is_err());
    let invalid = json!({"type":"mcl.execute.result","id":"turn-request","success":true,"data":{"kind":"turn","value":""}});
    assert!(serde_json::from_value::<ServerMessage>(invalid).is_err());
}

#[test]
fn compact_message_roundtrips_with_assistant_fields_and_keeps_compact_type() {
    let raw = json!({"type":"agent.message.full.sync","address":"/demo/coder","turn":null,"generation":null,"history_sequence":7,"message":{"type":"compact","sequence":7,"request_id":"summary-request","content":"summary","reasoning":"reason","tool_calls":[],"stop_reason":"completed"},"error":null});
    let message: ServerMessage = serde_json::from_value(raw).unwrap();
    let wire: Value = serde_json::from_str(&encode_server_message(&message).unwrap()).unwrap();
    assert_eq!(wire["message"]["type"], "compact");
    assert_eq!(wire["message"]["sequence"], 7);
    assert_eq!(wire["message"]["content"], "summary");
}

#[test]
fn model_file_requests_and_replies_keep_content_and_do_not_accept_a_client_path() {
    assert!(matches!(
        parse("model.config.req", json!({})).unwrap(),
        ClientMessage::ModelConfigReq { .. }
    ));
    assert!(matches!(
        parse("model.config.update", json!({"content":"[[models]]"})).unwrap(),
        ClientMessage::ModelConfigUpdate { .. }
    ));
    assert!(parse("model.config.req", json!({"path":"/tmp/other.toml"})).is_err());
    assert!(
        parse(
            "model.config.update",
            json!({"content":"text","path":"/tmp/other.toml"})
        )
        .is_err()
    );
    assert!(
        parse(
            "model.config.update",
            json!({"content":"x".repeat(1024*1024+1)})
        )
        .is_err()
    );
    for raw in [
        json!({"type":"model.config.res","id":"read","success":true,"content":"[[models]]"}),
        json!({"type":"model.config.res","id":"read-failed","success":false,"error":{"code":"InvalidModelRoute","message":"unreadable"}}),
        json!({"type":"model.config.sync","content":"[[models]]","error":null}),
        json!({"type":"model.config.sync","content":null,"error":{"code":"InvalidModelRoute","message":"unreadable"}}),
        json!({"type":"model.config.update.result","id":"write","success":true}),
    ] {
        let value: ServerMessage = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&encode_server_message(&value).unwrap()).unwrap(),
            raw
        );
    }
    assert!(
        serde_json::from_value::<ServerMessage>(
            json!({"type":"model.config.res","id":"missing-content","success":true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ServerMessage>(
            json!({"type":"model.config.sync","content":"source"})
        )
        .is_err()
    );
}

#[test]
fn agent_creator_is_a_workspace_or_agent_address_and_a_real_task_reference() {
    let mut raw = json!({"type":"agent.sync","agents":[{"address":"/demo/parent/child","workspace":"/demo","creator":{"address":"/demo/parent","task_id":"tool:1:2:0"},"status":"running","tools":[],"sandboxes":[],"sandbox":null,"model":null,"error":null}]});
    let message: ServerMessage = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), raw);
    raw["agents"][0]["creator"] = json!({"address":"/demo","task_id":"driver-task"});
    assert!(serde_json::from_value::<ServerMessage>(raw.clone()).is_ok());
    for creator in [
        json!({"address":"execution:1/driver:dev","task_id":"x"}),
        json!({"address":"/demo/parent","task_id":""}),
        json!({"address":"/demo/parent","task_id":"task","instance":"internal"}),
    ] {
        let mut invalid = raw.clone();
        invalid["agents"][0]["creator"] = creator;
        assert!(serde_json::from_value::<ServerMessage>(invalid).is_err());
    }
}

#[test]
fn creator_cannot_be_null_or_missing() {
    let valid = json!({"type":"agent.sync","agents":[{"address":"/demo/coder","workspace":"/demo","creator":{"address":"/demo","task_id":"driver-task"},"status":"starting","tools":[],"sandboxes":[],"sandbox":null,"model":null,"error":null}]});
    assert!(serde_json::from_value::<ServerMessage>(valid.clone()).is_ok());
    let mut null = valid.clone();
    null["agents"][0]["creator"] = Value::Null;
    assert!(serde_json::from_value::<ServerMessage>(null).is_err());
    let mut missing = valid;
    missing["agents"][0]
        .as_object_mut()
        .unwrap()
        .remove("creator");
    assert!(serde_json::from_value::<ServerMessage>(missing).is_err());
}

fn workspace_snapshot() -> Value {
    json!({"type":"workspace.sync","workspaces":[{
        "address":"/demo","alias":"Demo","project_root":"/project","status":"running",
        "agents":[{"id":"agent-1","parent_node":"/demo","node_name":"coder","alias":"Coder",
        "category":"normal","tool_path":"/tools/coder/tool.toml","tool_arguments":{"node_name":"coder","path":"."},
        "previous_node_name":null,"previous_alias":null}]
    }]})
}

#[test]
fn node_addresses_replace_entity_resource_tags_without_normalization() {
    for value in [
        "/",
        "/demo",
        "/demo/coder/reviewer",
        "/Demo/Some name",
        "/工作区/代理",
    ] {
        assert!(
            parse("agent.turn.req", json!({"address":value})).is_ok(),
            "{value}"
        );
    }
    for value in [
        "",
        "demo",
        "//demo",
        "/demo/",
        "/demo//coder",
        "/demo/../coder",
        "/demo/.",
        "/demo\\coder",
        "/demo:dev",
        "agent:scope/name:dev",
        "workspace:scope/name:dev",
        "/demo\n",
    ] {
        assert!(
            parse("agent.turn.req", json!({"address":value})).is_err(),
            "{value}"
        );
    }
}

#[test]
fn retired_workspace_lifecycle_requests_and_results_are_rejected() {
    for operation in ["start", "stop", "restart", "update", "rename"] {
        assert!(
            parse(
                &format!("workspace.{operation}"),
                json!({"address":"/demo"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<ServerMessage>(
                json!({"type":format!("workspace.{operation}.result"),"id":"r","success":true})
            )
            .is_err()
        );
    }
    assert!(parse("agent.restart", target()).is_err());
    assert!(
        serde_json::from_value::<ServerMessage>(
            json!({"type":"agent.restart.result","id":"r","success":true})
        )
        .is_err()
    );
}

#[test]
fn agent_start_variants_are_mutually_exclusive_and_driver_arguments_are_opaque() {
    let existing = json!({"mode":"existing","workspace":"/demo","node_name":"coder"});
    let new = json!({"mode":"new","workspace":"/demo","tool_path":"/tools/coder/tool.toml","arguments":{"node_name":"coder","alias":"Coder","path":".","custom":{"type":"assistant","replay":"driver-owned"}}});
    assert!(matches!(
        parse("agent.start", existing.clone()).unwrap(),
        ClientMessage::AgentStart {
            message: AgentStartInputDto::Existing { .. },
            ..
        }
    ));
    let parsed = parse("agent.start", new.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap()["message"], new);
    for invalid in [
        json!({"mode":"existing","workspace":"/demo","node_name":"coder","arguments":{}}),
        json!({"mode":"new","workspace":"/demo","tool_path":"/tools/coder","arguments":{},"node_name":"coder"}),
        json!({"mode":"new","workspace":"/demo","tool_path":"relative","arguments":{}}),
        json!({"mode":"new","workspace":"/demo","tool_path":"/tools/coder","arguments":[]}),
        json!({"mode":"existing","workspace":"/demo","node_name":"/demo/coder"}),
        json!({"mode":"existing","workspace":"/demo","node_name":"~temporary-id"}),
        json!({"mode":"new","workspace":"/demo","tool_path":"/tools/coder","arguments":{"node_name":""}}),
        json!({"mode":"unknown","workspace":"/demo","node_name":"coder"}),
    ] {
        assert!(parse("agent.start", invalid.clone()).is_err(), "{invalid}");
    }
    assert!(
        parse(
            "agent.start",
            json!({"mode":"new","workspace":"/demo","tool_path":"/tools/coder","arguments":{}})
        )
        .is_ok()
    );
}

#[test]
fn workspace_creation_is_a_single_root_node_and_rejects_old_definition_shape() {
    assert!(
        parse(
            "workspace.create",
            json!({"address":"/demo","alias":"Demo","project_root":"/project"})
        )
        .is_ok()
    );
    for message in [
        json!({"address":"/demo","definition":{"project_root":"/project","agents":[]}}),
        json!({"address":"/demo/child","alias":"Demo","project_root":"/project"}),
        json!({"address":"/","alias":"Demo","project_root":"/project"}),
        json!({"address":"/demo","alias":"Demo","project_root":"project"}),
    ] {
        assert!(parse("workspace.create", message).is_err());
    }
}

#[test]
fn workspace_registrations_are_strict_nullable_and_category_aware() {
    let valid = workspace_snapshot();
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<ServerMessage>(valid.clone()).unwrap())
            .unwrap(),
        valid
    );
    for key in ["node_name", "alias", "previous_node_name", "previous_alias"] {
        let mut v = valid.clone();
        v["workspaces"][0]["agents"][0]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(serde_json::from_value::<ServerMessage>(v).is_err(), "{key}");
    }
    for patch in [
        json!({"category":"running"}),
        json!({"category":"temporary"}),
        json!({"node_name":null}),
        json!({"tool_arguments":[]}),
        json!({"tool_path":"driver:scope/name:dev"}),
        json!({"legacy":true}),
    ] {
        let mut v = valid.clone();
        v["workspaces"][0]["agents"][0]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(
            serde_json::from_value::<ServerMessage>(v).is_err(),
            "{patch}"
        );
    }
    for category in ["temporary", "archived"] {
        let mut v = valid.clone();
        v["workspaces"][0]["agents"][0]["category"] = json!(category);
        v["workspaces"][0]["agents"][0]["node_name"] = Value::Null;
        assert!(serde_json::from_value::<ServerMessage>(v).is_ok());
    }
    let mut opaque = valid.clone();
    opaque["workspaces"][0]["agents"][0]["tool_arguments"] =
        json!({"custom":{"type":"assistant","replay":"preserved"}});
    assert!(serde_json::from_value::<ServerMessage>(opaque).is_ok());
    let mut duplicate = valid.clone();
    let row = duplicate["workspaces"][0]["agents"][0].clone();
    duplicate["workspaces"][0]["agents"]
        .as_array_mut()
        .unwrap()
        .push(row);
    assert!(serde_json::from_value::<ServerMessage>(duplicate).is_err());
}

#[test]
fn runtime_and_registration_states_cannot_be_confused() {
    let base = json!({"type":"agent.sync","agents":[{"address":"/demo/coder","workspace":"/demo","creator":{"address":"/demo","task_id":"driver-task"},"status":"starting","tools":[],"sandboxes":[],"sandbox":null,"model":null,"error":null}]});
    for state in ["starting", "running", "closing"] {
        let mut v = base.clone();
        v["agents"][0]["status"] = json!(state);
        assert!(serde_json::from_value::<ServerMessage>(v).is_ok());
    }
    for state in ["error", "stopping", "normal", "archived"] {
        let mut v = base.clone();
        v["agents"][0]["status"] = json!(state);
        assert!(serde_json::from_value::<ServerMessage>(v).is_err());
    }
}

#[test]
fn resource_filters_are_optional_exact_strings_and_never_null() {
    for filters in [
        json!({}),
        json!({"type":"driver"}),
        json!({"scope":"margatroid"}),
        json!({"name":"coder"}),
        json!({"type":"tool","scope":"margatroid","name":"bash"}),
    ] {
        let message = parse("resource.req", filters.clone()).unwrap();
        assert_eq!(serde_json::to_value(message).unwrap()["message"], filters);
    }
    for filters in [
        json!({"type":null}),
        json!({"scope":null}),
        json!({"name":null}),
        json!({"type":"Driver"}),
        json!({"type":"agent"}),
        json!({"type":""}),
        json!({"scope":""}),
        json!({"name":""}),
        json!({"scope":".."}),
        json!({"name":"a/b"}),
        json!({"scope":"a:b"}),
        json!({"name":"a\\b"}),
        json!({"scope":"a\nb"}),
        json!({"scoope":"margatroid"}),
    ] {
        assert!(parse("resource.req", filters.clone()).is_err(), "{filters}");
    }
}
fn resource(kind: &str) -> Value {
    let entry = match kind {
        "driver" => "base.lua",
        "tool" => "tool.toml",
        "prompt" => "prompt.toml",
        _ => "policy.json",
    };
    let path = format!("/resources/{kind}/margatroid/sample/dev/{entry}");
    json!({"address":format!("{kind}:margatroid/sample:dev"),"type":kind,"scope":"margatroid","name":"sample","tag":"dev","path":path,"tool_path":if kind=="tool"{Value::String(path)}else{Value::Null}})
}
#[test]
fn resource_response_roundtrip_enforces_entry_kind_and_nullable_presence() {
    for kind in ["driver", "tool", "prompt", "sandbox"] {
        let row = resource(kind);
        let value =
            json!({"type":"resource.res","id":"r","success":true,"resources":[row.clone()]});
        let parsed: ServerMessage = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
        assert_eq!(parsed.id(), Some("r"));
        let mut missing = value.clone();
        missing["resources"][0]
            .as_object_mut()
            .unwrap()
            .remove("tool_path");
        assert!(serde_json::from_value::<ServerMessage>(missing).is_err());
        for (key, bad) in [
            ("scope", json!("..")),
            ("name", json!("a/b")),
            ("tag", json!("")),
            ("path", json!("relative/base.lua")),
            ("path", json!("/resources/../base.lua")),
            ("address", json!("tool:wrong/sample:dev")),
            ("tool_path", json!("relative/tool.toml")),
        ] {
            let mut invalid = value.clone();
            invalid["resources"][0][key] = bad;
            assert!(
                serde_json::from_value::<ServerMessage>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }
    let mut driver = resource("driver");
    driver["tool_path"] = json!("/resources/driver/margatroid/sample/dev/tool.toml");
    assert!(
        serde_json::from_value::<ServerMessage>(
            json!({"type":"resource.res","id":"r","success":true,"resources":[driver]})
        )
        .is_ok()
    );
    for bad in [
        json!({"type":"resource.res","id":"r","success":true}),
        json!({"type":"resource.res","id":"r","success":true,"resources":null}),
        json!({"type":"resource.res","id":"r","success":false,"resources":[],"error":error()}),
    ] {
        assert!(serde_json::from_value::<ServerMessage>(bad).is_err());
    }
    let failure = json!({"type":"resource.res","id":"r","success":false,"error":error()});
    let parsed: ServerMessage = serde_json::from_value(failure.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), failure);
}

#[test]
fn resources_use_tuple_order_and_allow_canonical_symlink_targets() {
    let mut first = resource("driver");
    first["name"] = json!("a");
    first["address"] = json!("driver:margatroid/a:dev");
    first["path"] = json!("/resources/shared/actual.lua");
    first["tool_path"] = json!("/resources/entries/coder-manifest.toml");
    let mut second = resource("driver");
    second["name"] = json!("a-");
    second["address"] = json!("driver:margatroid/a-:dev");
    let value = json!({"type":"resource.res","id":"r","success":true,"resources":[first,second]});
    assert!(serde_json::from_value::<ServerMessage>(value.clone()).is_ok());
    let mut reversed = value;
    reversed["resources"].as_array_mut().unwrap().reverse();
    assert!(serde_json::from_value::<ServerMessage>(reversed).is_err());
}
