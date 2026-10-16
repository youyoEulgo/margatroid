use margatroid_protocol::{AgentMessageDto, IntoDomain, IntoDto, UserImage};
use margatroid_types::AgentMessage;
use serde_json::json;

#[test]
fn legacy_wire_and_nested_images_are_lossless() {
    let legacy = json!({"type":"user","content":"hello"});
    let dto: AgentMessageDto = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(&dto).unwrap(), legacy);
    let domain: AgentMessage = dto.into_domain(()).unwrap();
    let dto: AgentMessageDto = (&domain).into_dto(()).unwrap();
    assert_eq!(serde_json::to_value(dto).unwrap(), legacy);
    let wire = json!({"type":"inject","messages":[{"type":"user","content":"describe","images":[{"media_type":"image/png","data":"iVBORw0KGgo="}]}]});
    let dto: AgentMessageDto = serde_json::from_value(wire.clone()).unwrap();
    let domain: AgentMessage = dto.into_domain(()).unwrap();
    let dto: AgentMessageDto = (&domain).into_dto(()).unwrap();
    assert_eq!(serde_json::to_value(dto).unwrap(), wire);
}

#[test]
fn inbound_conversion_validates_even_when_dto_is_constructed_directly() {
    let data = "data:image/png;base64,iVBORw0KGgo=";
    let dto = AgentMessageDto::Inject {
        messages: vec![AgentMessageDto::User {
            content: String::new(),
            images: vec![UserImage {
                media_type: "image/png".into(),
                data: data.into(),
            }],
        }],
    };
    let result: Result<AgentMessage, _> = dto.into_domain(());
    let error = result.unwrap_err();
    assert!(error.message().contains("user image 1"));
    assert!(!error.message().contains(data));
}

#[test]
fn image_data_does_not_consume_non_image_message_budget() {
    let mut data = "iVBORw0KGgoA".to_owned();
    data.push_str(&"A".repeat(4 * 400_000));
    let dto = AgentMessageDto::User {
        content: String::new(),
        images: vec![UserImage {
            media_type: "image/png".into(),
            data,
        }],
    };
    let domain: AgentMessage = dto.into_domain(()).unwrap();
    assert!(
        matches!(domain, AgentMessage::User { images, .. } if images[0].data.len() > 1024 * 1024)
    );
    let dto = AgentMessageDto::User {
        content: "x".repeat(1024 * 1024),
        images: vec![],
    };
    let result: Result<AgentMessage, _> = dto.into_domain(());
    assert!(result.unwrap_err().message().contains("non-image"));
}

#[test]
fn tool_result_images_round_trip_validate_and_have_separate_budget() {
    let legacy = json!({"type":"tool_call","resource_id":null,"tool_call_id":"read","tool_name":"read_image","content":"loaded","failed":false});
    let dto: AgentMessageDto = serde_json::from_value(legacy.clone()).unwrap();
    let domain: AgentMessage = dto.into_domain(()).unwrap();
    let dto: AgentMessageDto = (&domain).into_dto(()).unwrap();
    assert_eq!(serde_json::to_value(dto).unwrap(), legacy);
    let mut tool = legacy;
    tool["images"] = json!([{"media_type":"image/png","data":"iVBORw0KGgo="}]);
    let wire = json!({"type":"inject","messages":[tool.clone()]});
    let dto: AgentMessageDto = serde_json::from_value(wire.clone()).unwrap();
    let domain: AgentMessage = dto.into_domain(()).unwrap();
    let dto: AgentMessageDto = (&domain).into_dto(()).unwrap();
    assert_eq!(serde_json::to_value(dto).unwrap(), wire);
    tool["images"][0]["data"] = format!("iVBORw0KGgoA{}", "A".repeat(4 * 400_000)).into();
    let dto: AgentMessageDto = serde_json::from_value(tool).unwrap();
    let _: AgentMessage = dto.into_domain(()).unwrap();
    let dto = AgentMessageDto::ToolCall {
        resource_id: None,
        tool_call_id: "read".into(),
        tool_name: "read_image".into(),
        content: String::new(),
        images: vec![UserImage {
            media_type: "image/png".into(),
            data: "secret_invalid".into(),
        }],
        failed: false,
    };
    let result: Result<AgentMessage, _> = dto.into_domain(());
    let error = result.unwrap_err();
    assert!(error.message().contains("user image 1"));
    assert!(!error.message().contains("secret_invalid"));
}
