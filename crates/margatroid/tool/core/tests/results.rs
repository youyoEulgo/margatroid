use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tool_core::{
    InitToolTasks, ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecutionLimits,
    ToolTaskHandle, parse_tool_result,
};

fn parse(value: &Value) -> tool_core::ToolExecuteResponse {
    parse_tool_result(value, &ToolExecutionLimits::default()).unwrap()
}

#[test]
fn strings_are_literal_and_other_plain_values_are_json() {
    for text in [
        "plain text",
        "你好\nworld",
        "{\"ok\":false,\"content\":\"literal\"}",
    ] {
        let response = parse(&json!(text));
        assert_eq!(response.content, text);
        assert!(response.call_id.is_empty());
        assert!(response.alias.is_empty());
        assert!(response.images.is_empty());
        assert!(!response.failed);
    }
    for value in [
        json!(null),
        json!(42),
        json!(true),
        json!([1, "two"]),
        json!({"other": true}),
        json!({"ok": "yes", "content": "not envelope"}),
    ] {
        assert_eq!(parse(&value).content, value.to_string());
    }
}

#[test]
fn envelopes_preserve_reported_failure_and_result_text() {
    let success = parse(&json!({"ok": true, "content": "done"}));
    assert_eq!(success.content, "done");
    assert!(!success.failed);
    let failed = parse(&json!({"ok": false, "content": "denied", "images": []}));
    assert_eq!(failed.content, "denied");
    assert!(failed.failed);
}

#[test]
fn images_require_typed_envelopes_canonical_base64_and_matching_signatures() {
    for (media_type, bytes) in [
        ("image/png", b"\x89PNG\r\n\x1a\n".as_slice()),
        ("image/jpeg", b"\xff\xd8\xff".as_slice()),
        ("image/gif", b"GIF89a".as_slice()),
        ("image/webp", b"RIFFxxxxWEBP".as_slice()),
    ] {
        let data = STANDARD.encode(bytes);
        let response = parse(
            &json!({"ok": true, "content": "image", "images": [{"media_type": media_type, "data": data}]}),
        );
        assert_eq!(response.images[0].media_type, media_type);
        assert_eq!(response.images[0].data, data);
    }
    for images in [
        json!(null),
        json!({}),
        json!([{}]),
        json!([{"media_type": "image/png", "data": "SECRET_NOT_BASE64"}]),
        json!([{"media_type": "image/png", "data": ""}]),
        json!([{"media_type": "image/png", "data": "AA"}]),
        json!([{"media_type": "image/png", "data": "===="}]),
        json!([{"media_type": "image/png", "data": "AB=="}]),
        json!([{"media_type": "image/png", "data": "aGVsbG8="}]),
        json!([{"media_type": "text/plain", "data": "aGVsbG8="}]),
    ] {
        let error = parse_tool_result(
            &json!({"ok": true, "content": "image", "images": images}),
            &ToolExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ToolErrorKind::RunnerProtocolFailed);
        assert!(!error.message.contains("SECRET_NOT_BASE64"));
    }
    for invalid in [
        json!({"images": []}),
        json!({"ok": true, "images": []}),
        json!({"ok": false, "content": "failed", "images": [{"media_type": "image/png", "data": STANDARD.encode(b"\x89PNG\r\n\x1a\n")} ]}),
    ] {
        assert_eq!(
            parse_tool_result(&invalid, &ToolExecutionLimits::default())
                .unwrap_err()
                .kind,
            ToolErrorKind::RunnerProtocolFailed
        );
    }
}

#[test]
fn text_and_total_result_limits_enforce_exact_encoded_boundaries() {
    let plain = json!("界");
    let exact = ToolExecutionLimits {
        output_bytes: 3,
        result_bytes: 5,
        ..ToolExecutionLimits::default()
    };
    assert_eq!(parse_tool_result(&plain, &exact).unwrap().content, "界");
    assert_eq!(
        parse_tool_result(
            &plain,
            &ToolExecutionLimits {
                output_bytes: 2,
                ..exact.clone()
            }
        )
        .unwrap_err()
        .kind,
        ToolErrorKind::OutputTooLarge
    );
    assert_eq!(
        parse_tool_result(
            &plain,
            &ToolExecutionLimits {
                result_bytes: 4,
                ..exact
            }
        )
        .unwrap_err()
        .kind,
        ToolErrorKind::OutputTooLarge
    );
    let envelope = json!({"ok": true, "content": "abc"});
    let length = envelope.to_string().len();
    let limits = ToolExecutionLimits {
        output_bytes: length,
        result_bytes: length,
        ..ToolExecutionLimits::default()
    };
    assert!(parse_tool_result(&envelope, &limits).is_ok());
    assert_eq!(
        parse_tool_result(
            &envelope,
            &ToolExecutionLimits {
                output_bytes: length - 1,
                ..limits.clone()
            }
        )
        .unwrap_err()
        .kind,
        ToolErrorKind::OutputTooLarge
    );
    assert_eq!(
        parse_tool_result(
            &envelope,
            &ToolExecutionLimits {
                result_bytes: length - 1,
                ..limits
            }
        )
        .unwrap_err()
        .kind,
        ToolErrorKind::OutputTooLarge
    );
}

#[test]
fn image_count_and_decoded_byte_budgets_are_enforced() {
    let image = json!({"media_type": "image/png", "data": STANDARD.encode(b"\x89PNG\r\n\x1a\n")});
    assert_eq!(
        parse(&json!({"ok": true, "content": "", "images": vec![image.clone(); 8]}))
            .images
            .len(),
        8
    );
    assert_eq!(
        parse_tool_result(
            &json!({"ok": true, "content": "", "images": vec![image; 9]}),
            &ToolExecutionLimits::default()
        )
        .unwrap_err()
        .kind,
        ToolErrorKind::RunnerProtocolFailed
    );
    let mut bytes = vec![0; tool_core::MAX_TOOL_IMAGE_BYTES + 1];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let oversized = json!({"ok": true, "content": "", "images": [{"media_type": "image/png", "data": STANDARD.encode(&bytes)}]});
    assert_eq!(
        parse_tool_result(&oversized, &ToolExecutionLimits::default())
            .unwrap_err()
            .kind,
        ToolErrorKind::OutputTooLarge
    );
    bytes.pop();
    let image = json!({"media_type": "image/png", "data": STANDARD.encode(&bytes)});
    let limits = ToolExecutionLimits {
        result_bytes: 40 * 1024 * 1024,
        ..ToolExecutionLimits::default()
    };
    assert!(
        parse_tool_result(
            &json!({"ok": true, "content": "", "images": vec![image.clone(); 4]}),
            &limits
        )
        .is_ok()
    );
    assert_eq!(
        parse_tool_result(
            &json!({"ok": true, "content": "", "images": vec![image; 5]}),
            &limits
        )
        .unwrap_err()
        .kind,
        ToolErrorKind::OutputTooLarge
    );
}

#[test]
fn image_payload_does_not_consume_text_budget_but_counts_towards_total() {
    let value = json!({"ok": true, "content": "summary", "images": [{"media_type": "image/png", "data": STANDARD.encode(b"\x89PNG\r\n\x1a\n")} ]});
    let limits = ToolExecutionLimits {
        output_bytes: json!({"ok": true, "content": "summary"}).to_string().len(),
        result_bytes: value.to_string().len(),
        ..ToolExecutionLimits::default()
    };
    assert!(parse_tool_result(&value, &limits).is_ok());
    assert_eq!(
        parse_tool_result(
            &value,
            &ToolExecutionLimits {
                result_bytes: value.to_string().len() - 1,
                ..limits
            }
        )
        .unwrap_err()
        .kind,
        ToolErrorKind::OutputTooLarge
    );
}

#[test]
fn execution_limits_validate_and_round_trip() {
    let defaults = ToolExecutionLimits::default();
    defaults.validate().unwrap();
    assert_eq!(
        serde_json::from_value::<ToolExecutionLimits>(serde_json::to_value(&defaults).unwrap())
            .unwrap(),
        defaults
    );
    let mut invalids = Vec::new();
    for field in 0..8 {
        let mut invalid = defaults.clone();
        match field {
            0 => invalid.script_bytes = 0,
            1 => invalid.argument_bytes = 0,
            2 => invalid.output_bytes = 0,
            3 => invalid.result_bytes = 0,
            4 => invalid.diagnostic_bytes = 0,
            5 => invalid.memory_bytes = 0,
            6 => invalid.cache_bytes = 0,
            _ => invalid.input_capacity = 0,
        }
        invalids.push(invalid);
    }
    invalids.extend([
        ToolExecutionLimits {
            instruction_budget: 0,
            ..defaults.clone()
        },
        ToolExecutionLimits {
            input_capacity: usize::MAX,
            ..defaults
        },
    ]);
    for invalid in invalids {
        assert_eq!(
            invalid.validate().unwrap_err().kind,
            ToolErrorKind::InvalidRequest
        );
        assert_eq!(
            ToolTaskHandle::new(&invalid, "call".into(), "tool".into(), oneshot::channel().0)
                .unwrap_err()
                .kind,
            ToolErrorKind::InvalidRequest
        );
    }
}

#[tokio::test]
async fn events_share_single_claimable_oneshot_with_turn_identity() {
    fn event<T: core_plugin::Event>() {}
    event::<InitToolTasks>();
    event::<ToolExecuteRequest>();
    let agent = core_plugin::World::new().spawn();
    let (sender, receiver) = oneshot::channel();
    let init = InitToolTasks {
        agent,
        reply: Arc::new(Mutex::new(Some(sender))),
    };
    let clone = init.clone();
    clone
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(()))
        .unwrap();
    assert!(init.reply.lock().unwrap().is_none());
    receiver.await.unwrap().unwrap();
    let (sender, receiver) = oneshot::channel();
    let request = ToolExecuteRequest {
        id: "task".into(),
        agent,
        turn: "turn".into(),
        alias: "tool".into(),
        call_id: "model-call".into(),
        arguments: json!({"value": 1}),
        reply: Arc::new(Mutex::new(Some(sender))),
    };
    let clone = request.clone();
    clone
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(parse(&json!("done"))))
        .unwrap();
    assert!(request.reply.lock().unwrap().is_none());
    assert_eq!(receiver.await.unwrap().unwrap().content, "done");
}

#[test]
fn errors_are_bounded_and_serializable_without_other_domains() {
    let error = ToolError::new(ToolErrorKind::CleanupFailed, "界".repeat(1000));
    assert_eq!(error.message.chars().count(), 512);
    let standard: &dyn std::error::Error = &error;
    assert!(standard.to_string().starts_with("CleanupFailed:"));
    assert_eq!(
        serde_json::from_value::<ToolError>(serde_json::to_value(&error).unwrap()).unwrap(),
        error
    );
}
