use margatroid_protocol::{
    ClientInfoDto, ClientMessage, IntoDomain, MessageDto, ProtocolErrorKind, ResourceIdDto,
    ServerMessage, ToolCallDto,
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
fn clients_may_inject_context_carrying_any_message_but_an_injection() {
    let injected = MessageDto::Inject {
        messages: vec![
            MessageDto::User {
                content: "please review".into(),
            },
            MessageDto::Assistant {
                reasoning: None,
                content: None,
                tool_calls: vec![ToolCallDto {
                    id: "manual-1".into(),
                    tool_name: "code_review".into(),
                    arguments: "{}".into(),
                }],
            },
        ],
    };
    let message = injected.into_domain(()).unwrap();
    let Message::Inject { messages } = message else {
        panic!("expected a context injection")
    };
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], Message::User { .. }));
    assert!(matches!(messages[1], Message::Assistant { .. }));

    let nested = MessageDto::Inject {
        messages: vec![MessageDto::Inject { messages: vec![] }],
    };
    let error = nested.into_domain(()).unwrap_err();
    assert!(matches!(error.kind(), ProtocolErrorKind::InvalidRequest));

    let server_side = MessageDto::Inject {
        messages: vec![
            MessageDto::Tool {
                resource_id: ResourceIdDto("tool:local/read-file:latest".into()),
                tool_call_id: "call-1".into(),
                content: "recorded".into(),
            },
            MessageDto::Error {
                message: "recorded failure".into(),
            },
        ],
    };
    let message = server_side.into_domain(()).unwrap();
    let Message::Inject { messages } = message else {
        panic!("expected a context injection")
    };
    assert!(matches!(messages[0], Message::Tool { .. }));
    assert!(matches!(messages[1], Message::Error { .. }));
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

#[test]
fn registration_receipts_use_stable_server_shapes() {
    let registered = serde_json::to_value(ServerMessage::ConnectionRegistered {
        id: "register-1".into(),
        client: ClientInfoDto {
            resource_id: ResourceIdDto("client:webui/console:1".into()),
            client_type: "webui".into(),
            name: "console".into(),
        },
    })
    .unwrap();
    assert_eq!(registered["type"], "connection.registered");
    assert_eq!(registered["id"], "register-1");
    assert_eq!(
        registered["client"]["resource_id"],
        "client:webui/console:1"
    );
    assert_eq!(registered["client"]["client_type"], "webui");
    assert_eq!(registered["client"]["name"], "console");

    let failed = serde_json::to_value(ServerMessage::ConnectionRegisterFailed {
        id: "register-2".into(),
        error: "client type is not a stable identifier".into(),
    })
    .unwrap();
    assert_eq!(failed["type"], "connection.register_failed");
    assert_eq!(failed["id"], "register-2");
    assert_eq!(failed["error"], "client type is not a stable identifier");
}
