use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tool_core::{
    InitToolTasks, ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecutionLimits,
    ToolTaskHandle, parse_tool_result,
};

fn parse(value: &Value) -> tool_core::ToolExecuteResponse {
    parse_tool_result(value).unwrap()
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
        let error = parse_tool_result(&json!({"ok": true, "content": "image", "images": images}))
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
            parse_tool_result(&invalid).unwrap_err().kind,
            ToolErrorKind::RunnerProtocolFailed
        );
    }
}

#[test]
fn result_text_is_preserved_in_plain_values_and_envelopes() {
    let text = "你好\n".repeat(1024);
    assert_eq!(parse_tool_result(&json!(text)).unwrap().content, text);
    let envelope = json!({"ok": true, "content": text});
    assert_eq!(parse_tool_result(&envelope).unwrap().content, text);
}

#[test]
fn host_accepts_nine_images_with_complete_payloads() {
    let mut bytes = vec![0; 1024];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let data = STANDARD.encode(&bytes);
    let image = json!({"media_type": "image/png", "data": data});
    let value = json!({"ok": true, "content": "summary", "images": vec![image; 9]});
    let response = parse_tool_result(&value).unwrap();
    assert_eq!(response.content, "summary");
    assert_eq!(response.images.len(), 9);
    assert!(response.images.iter().all(|image| image.data == data));
}

#[test]
fn host_accepts_large_images_and_aggregate_image_payloads() {
    let mut bytes = vec![0; 4 * 1024 * 1024 + 1];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let data = STANDARD.encode(&bytes);
    let image = json!({"media_type": "image/png", "data": data});
    let value = json!({"ok": true, "content": "large images", "images": vec![image; 5]});
    let response = parse_tool_result(&value).unwrap();
    assert_eq!(response.images.len(), 5);
    assert!(response.images.iter().all(|image| image.data == data));
}

#[test]
fn execution_limits_have_lua_memory_and_buffer_budgets_and_round_trip() {
    let defaults = ToolExecutionLimits::default();
    defaults.validate().unwrap();
    let value = serde_json::to_value(&defaults).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 2);
    assert_eq!(value["lua_vm_memory_bytes"], defaults.lua_vm_memory_bytes);
    assert!(
        serde_json::from_value::<ToolExecutionLimits>(
            serde_json::json!({"memory_bytes": 1024, "cache_bytes":1024})
        )
        .is_err()
    );
    assert_eq!(value["cache_bytes"], defaults.cache_bytes);
    assert_eq!(
        serde_json::from_value::<ToolExecutionLimits>(value).unwrap(),
        defaults
    );
    for invalid in [
        ToolExecutionLimits {
            lua_vm_memory_bytes: 0,
            ..defaults.clone()
        },
        ToolExecutionLimits {
            cache_bytes: 0,
            ..defaults.clone()
        },
    ] {
        assert_eq!(
            invalid.validate().unwrap_err().kind,
            ToolErrorKind::InvalidRequest
        );
        assert_eq!(
            ToolTaskHandle::new(
                &invalid,
                "call".into(),
                "tool".into(),
                oneshot::channel().0,
                test_tasks().update_context()
            )
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

fn test_binding() -> tool_core::TaskSyncBinding {
    tool_core::TaskSyncBinding::new(
        client_core::Clients::default(),
        std::sync::Arc::from([]),
        |snapshot| {
            serde_json::to_string(&snapshot)
                .map(|text| std::sync::Arc::new(client_core::ClientFrame::Text(text)))
                .map_err(|_| client_core::ClientPrepareError::EncodeFailed)
        },
    )
}
fn test_tasks() -> tool_core::ToolTasks {
    let mut app = core_plugin::App::new();
    let owner = app.world_mut().spawn();
    tool_core::ToolTasks::new(owner, "agent:test/unit:latest".into(), test_binding())
}
