use margatroid_protocol::{ServerMessage, encode_server_message};
use serde_json::{Value, json};

fn chunk() -> Value {
    json!({"type":"agent.message.chunk.sync","address":"/demo/assistant","turn":null,"tag":"compact","generation":"summary-1","sequence":1,"content":"summary text","reasoning":""})
}
fn full() -> Value {
    json!({"type":"agent.message.full.sync","address":"/demo/assistant","turn":null,"tag":"compact","generation":"summary-1","history_sequence":null,"message":{"type":"assistant","content":"summary text","tool_calls":[]},"error":null})
}
fn roundtrip(value: &Value) {
    let message: ServerMessage = serde_json::from_value(value.clone()).unwrap();
    let encoded = encode_server_message(&message).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), *value);
}

#[test]
fn tagged_compaction_chunks_and_success_or_error_full_messages_roundtrip() {
    roundtrip(&chunk());
    roundtrip(&full());
    let mut error = full();
    error["message"] = json!({"type":"error","code":"RequestBodyTooLarge","message":"HTTP 413","request_id":"summary-1","details":{"http_status":413}});
    error["error"] = json!({"code":"RequestBodyTooLarge","message":"HTTP 413"});
    roundtrip(&error);
    assert!(error["message"].get("tag").is_none());
}

#[test]
fn generation_tag_is_required_nullable_and_validated_without_trimming() {
    for value in [chunk(), full()] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("tag");
        assert!(serde_json::from_value::<ServerMessage>(missing).is_err());
        for tag in [
            json!(null),
            json!("compact"),
            json!(" label "),
            json!("压缩"),
        ] {
            let mut candidate = value.clone();
            candidate["tag"] = tag;
            roundtrip(&candidate);
        }
        for tag in [
            json!(""),
            json!(" \t "),
            json!("bad\nlabel"),
            json!("x".repeat(513)),
            json!(false),
            json!(42),
            json!({}),
            json!([]),
        ] {
            let mut candidate = value.clone();
            candidate["tag"] = tag;
            assert!(serde_json::from_value::<ServerMessage>(candidate).is_err());
        }
    }
}

#[test]
fn nongenerated_full_messages_require_null_tag_and_preserve_history_message_shape() {
    let mut value = full();
    value["generation"] = Value::Null;
    value["tag"] = Value::Null;
    value["history_sequence"] = json!(7);
    value["message"] = json!({"type":"user","sequence":7,"content":"persisted","data":[]});
    roundtrip(&value);
    value["tag"] = json!("compact");
    assert!(serde_json::from_value::<ServerMessage>(value).is_err());
}
