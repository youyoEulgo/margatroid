use margatroid_protocol::{
    ClientMessage, IntoDomain, MessageDto, ProtocolErrorKind, ResourceIdDto, ToolCallDto,
};
use margatroid_types::{Message, ToolCall};

fn assistant(tool_name: &str) -> MessageDto {
    MessageDto::Assistant {
        reasoning: None,
        content: None,
        tool_calls: vec![ToolCallDto {
            id: "manual-1".into(),
            tool_name: tool_name.into(),
            arguments: "{}".into(),
        }],
    }
}

#[test]
fn clients_may_inject_user_and_assistant_messages() {
    assert_eq!(
        MessageDto::User {
            content: "hello".into(),
        }
        .into_domain(())
        .unwrap(),
        Message::User {
            content: "hello".into(),
        }
    );
    assert_eq!(
        assistant("code_review").into_domain(()).unwrap(),
        Message::Assistant {
            reasoning: None,
            content: None,
            tool_calls: vec![ToolCall {
                id: "manual-1".into(),
                tool_name: "code_review".into(),
                arguments: "{}".into(),
            }],
        }
    );
}

#[test]
fn clients_may_not_inject_server_side_facts() {
    let rejected = [
        MessageDto::Tool {
            resource_id: ResourceIdDto("tool:local/read-file:latest".into()),
            tool_call_id: "call-1".into(),
            content: "contents".into(),
        },
        MessageDto::Error {
            message: "boom".into(),
        },
    ];
    for message in rejected {
        let error = message.into_domain(()).unwrap_err();
        assert!(matches!(error.kind(), ProtocolErrorKind::InvalidRequest));
    }
}

#[test]
fn the_standalone_assistant_message_type_is_gone() {
    let request = serde_json::json!({
        "type": "agent.assistant",
        "id": "manual-1",
        "message": {
            "workspace": {
                "id": "workspace:local/demo:latest",
                "name": "demo",
                "project_root": "/tmp/demo"
            },
            "agent": null,
            "content": null,
            "reasoning": null,
            "tool_calls": []
        }
    });
    assert!(serde_json::from_value::<ClientMessage>(request).is_err());
}
