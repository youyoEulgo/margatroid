use base64::{engine::general_purpose::STANDARD, Engine as _};
use margatroid_types::message_types::{MAX_USER_IMAGES_BYTES, MAX_USER_IMAGE_BYTES};
use margatroid_types::{validate_user_images, AgentMessage, UserImage};
use serde_json::json;

fn image(media_type: &str, bytes: &[u8]) -> UserImage {
    UserImage {
        media_type: media_type.into(),
        data: STANDARD.encode(bytes),
    }
}

#[test]
fn legacy_user_wire_is_unchanged_and_images_round_trip_with_history() {
    let legacy = json!({"type":"user","content":"hello"});
    let message: AgentMessage = serde_json::from_value(legacy.clone()).unwrap();
    assert!(matches!(&message, AgentMessage::User { images, .. } if images.is_empty()));
    assert_eq!(serde_json::to_value(&message).unwrap(), legacy);
    let message = AgentMessage::User {
        content: "describe".into(),
        images: vec![image("image/png", b"\x89PNG\r\n\x1a\n")],
    };
    let encoded = serde_json::to_value(&message).unwrap();
    assert_eq!(
        serde_json::from_value::<AgentMessage>(encoded).unwrap(),
        message
    );
    let (content, mut payload) =
        margatroid_types::history_components::message_history_parts(&message);
    assert_eq!(content, "describe");
    assert!(payload.get("content").is_none());
    assert!(payload.get("images").is_some());
    payload["content"] = content.into();
    assert_eq!(
        serde_json::from_value::<AgentMessage>(payload).unwrap(),
        message
    );
}

#[test]
fn signatures_accept_all_supported_formats_without_full_decoding() {
    for valid in [
        image("image/png", b"\x89PNG\r\n\x1a\n"),
        image("image/jpeg", b"\xff\xd8\xff"),
        image("image/gif", b"GIF87a"),
        image("image/gif", b"GIF89a"),
        image("image/webp", b"RIFF\x04\0\0\0WEBP"),
    ] {
        validate_user_images(&[valid]).unwrap();
    }
}

#[test]
fn invalid_encodings_signatures_and_mime_fail_without_payload_leaks() {
    let cases = [
        UserImage {
            media_type: "image/png".into(),
            data: "data:image/png;base64,iVBORw0KGgo=".into(),
        },
        UserImage {
            media_type: "image/png".into(),
            data: "not valid base64!".into(),
        },
        UserImage {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo".into(),
        },
        UserImage {
            media_type: "image/png".into(),
            data: "iVBORw0KGgp=".into(),
        },
        UserImage {
            media_type: "image/png".into(),
            data: "____".into(),
        },
        image("image/jpeg", b"\x89PNG\r\n\x1a\n"),
        image("image/avif", b"not a supported format"),
        image("image/webp", b"RIFF\0\0\0\0NOPE"),
        image("image/gif", b"GIF89"),
    ];
    for invalid in cases {
        let error = validate_user_images(std::slice::from_ref(&invalid)).unwrap_err();
        assert!(error.contains("user image 1"));
        assert!(!error.contains(&invalid.data));
        let wire = json!({"type":"user","content":"","images":[invalid]});
        let error = serde_json::from_value::<AgentMessage>(wire)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(&invalid.data));
    }
    assert!(validate_user_images(&[UserImage {
        media_type: "image/png".into(),
        data: String::new()
    }])
    .is_err());
}

#[test]
fn limits_accept_exact_boundaries_and_reject_overages() {
    let mut bytes = vec![0; MAX_USER_IMAGE_BYTES];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let full = image("image/png", &bytes);
    validate_user_images(&vec![
        full.clone();
        MAX_USER_IMAGES_BYTES / MAX_USER_IMAGE_BYTES
    ])
    .unwrap();
    bytes.push(0);
    assert!(validate_user_images(&[image("image/png", &bytes)])
        .unwrap_err()
        .contains("5 MiB"));
    let mut total = vec![full; 4];
    total.push(image("image/gif", b"GIF89a"));
    assert!(validate_user_images(&total).unwrap_err().contains("20 MiB"));
    let small = image("image/gif", b"GIF89a");
    validate_user_images(&vec![small.clone(); 8]).unwrap();
    assert!(validate_user_images(&vec![small; 9])
        .unwrap_err()
        .contains("8 images"));
    let encoded_oversize = UserImage {
        media_type: "image/png".into(),
        data: "!".repeat(MAX_USER_IMAGE_BYTES.div_ceil(3) * 4 + 1),
    };
    assert!(validate_user_images(&[encoded_oversize])
        .unwrap_err()
        .contains("5 MiB"));
}

#[test]
fn tool_result_images_preserve_legacy_wire_and_history() {
    let legacy = json!({"type":"tool_call","resource_id":null,"tool_call_id":"read","tool_name":"read_image","content":"loaded","failed":false});
    let message: AgentMessage = serde_json::from_value(legacy.clone()).unwrap();
    assert!(matches!(&message, AgentMessage::ToolCall { images, .. } if images.is_empty()));
    assert_eq!(serde_json::to_value(message).unwrap(), legacy);
    let mut wire = legacy;
    wire["images"] = json!([image("image/png", b"\x89PNG\r\n\x1a\n")]);
    let message: AgentMessage = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&message).unwrap(), wire);
    let (content, mut payload) =
        margatroid_types::history_components::message_history_parts(&message);
    assert_eq!(content, "loaded");
    assert_eq!(payload["images"], wire["images"]);
    assert!(payload.get("content").is_none());
    payload["content"] = content.into();
    assert_eq!(
        serde_json::from_value::<AgentMessage>(payload).unwrap(),
        message
    );
    for images in [
        json!([{"media_type":"image/png","data":"secret_invalid"}]),
        json!(vec![image("image/png", b"\x89PNG\r\n\x1a\n"); 9]),
    ] {
        wire["images"] = images;
        let error = serde_json::from_value::<AgentMessage>(wire.clone())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret_invalid"));
    }
}
