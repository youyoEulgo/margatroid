use margatroid_protocol::{
    ClientMessage, ServerMessage, encode_server_message, parse_client_message,
};
use serde_json::json;

#[test]
fn large_mcl_request_and_message_result_have_no_envelope_byte_limit() {
    const FORMER_LIMIT: usize = 32 * 1024 * 1024;
    let content = "x".repeat(FORMER_LIMIT + 1);
    let text = json!({
        "type":"mcl.execute", "id":"large-request",
        "message":{
            "address":"/demo/coder", "command":"INJECT ? TO session.conversation[-1]",
            "arguments":[[{"type":"user","content":content}]]
        }
    })
    .to_string();
    assert!(text.len() > FORMER_LIMIT);
    let parsed =
        parse_client_message(&text).expect("valid requests are not limited by envelope bytes");
    let ClientMessage::MclExecute { message, .. } = parsed else {
        panic!("expected MCL request")
    };
    assert_eq!(
        message.arguments[0][0]["content"].as_str().unwrap().len(),
        FORMER_LIMIT + 1
    );
    drop(text);
    drop(message);

    let server: ServerMessage = serde_json::from_value(json!({
        "type":"mcl.execute.result", "id":"large-result", "success":true,
        "data":{"kind":"messages","value":[{"type":"user","content":content}]}
    }))
    .unwrap();
    let text =
        encode_server_message(&server).expect("complete Block results have no envelope byte limit");
    assert!(text.len() > FORMER_LIMIT);
    let decoded: ServerMessage = serde_json::from_str(&text).expect("large results roundtrip");
    assert_eq!(
        serde_json::to_value(decoded).unwrap()["data"]["value"][0]["content"]
            .as_str()
            .unwrap()
            .len(),
        FORMER_LIMIT + 1
    );
}

#[test]
fn inference_error_preserves_machine_code_and_size_details() {
    let value = json!({"type":"mcl.execute.result","id":"budget","success":true,"data":{"kind":"inference","value":{"message":{"type":"error","message":"request body too large","request_id":"original","code":"RequestBodyTooLarge","details":{"http_status":413,"request_id":"upstream-id"}},"stop_reason":"error"}}});
    let message: ServerMessage = serde_json::from_value(value.clone()).unwrap();
    let encoded = encode_server_message(&message).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
        value
    );
}
