use margatroid_protocol::*;
use serde_json::{Value, json};

fn parse(kind: &str, message: Value) -> Result<ClientMessage, ProtocolError> {
    parse_client_message(&json!({"type":kind,"id":"rename-request","message":message}).to_string())
}

#[test]
fn alias_requests_roundtrip_and_preserve_opaque_ids_and_untrimmed_names() {
    let request = parse(
        "workspace.rename",
        json!({"address":"/demo","alias":"  中文项目  "}),
    )
    .unwrap();
    assert_eq!(request.id(), "rename-request");
    assert!(
        matches!(&request, ClientMessage::WorkspaceRename { message, .. } if message.alias == "  中文项目  ")
    );
    assert_eq!(
        parse_client_message(&serde_json::to_string(&request).unwrap()).unwrap(),
        request
    );
    for alias in [json!(null), json!("  开发助手  "), json!("a".repeat(512))] {
        let payload =
            json!({"workspace":"/demo","agent_id":"opaque/id: with spaces-非hex","alias":alias});
        let request = parse("agent.rename", payload.clone()).unwrap();
        assert_eq!(request.id(), "rename-request");
        assert!(
            matches!(&request, ClientMessage::AgentRename { message, .. } if message.agent_id == "opaque/id: with spaces-非hex")
        );
        assert_eq!(serde_json::to_value(&request).unwrap()["message"], payload);
        assert_eq!(
            parse_client_message(&serde_json::to_string(&request).unwrap()).unwrap(),
            request
        );
    }
    assert!(
        parse(
            "workspace.rename",
            json!({"address":"/demo","alias":"a".repeat(512)})
        )
        .is_ok()
    );
    assert!(
        parse(
            "agent.rename",
            json!({"workspace":"/demo","agent_id":"i".repeat(512),"alias":null})
        )
        .is_ok()
    );
}

#[test]
fn alias_request_shapes_reject_unknown_fields_and_missing_nullable_alias() {
    for payload in [
        json!({"address":"/demo"}),
        json!({"address":"/demo","alias":null}),
        json!({"address":"/demo","alias":4}),
        json!({"address":"/demo","alias":"ok","node_name":"new"}),
        json!({"workspace":"/demo","alias":"ok"}),
    ] {
        assert!(parse("workspace.rename", payload).is_err());
    }
    for payload in [
        json!({"workspace":"/demo","agent_id":"id"}),
        json!({"workspace":"/demo","alias":null}),
        json!({"workspace":"/demo","agent_id":null,"alias":null}),
        json!({"address":"/demo/agent","alias":"new"}),
        json!({"workspace":"/demo","agent_id":"id","alias":5}),
        json!({"workspace":"/demo","agent_id":"id","alias":"ok","node_name":"new"}),
    ] {
        assert!(parse("agent.rename", payload.clone()).is_err(), "{payload}");
        assert!(serde_json::from_value::<AgentRenameDto>(payload).is_err());
    }
    // The mandatory nullable field is enforced by derived serde, not just the wire parser.
    assert!(
        serde_json::from_value::<AgentRenameDto>(
            json!({"workspace":"/demo","agent_id":"id","alias":null})
        )
        .unwrap()
        .alias
        .is_none()
    );
    for kind in ["workspace.rename", "agent.rename"] {
        let message = if kind == "workspace.rename" {
            json!({"address":"/demo","alias":"ok"})
        } else {
            json!({"workspace":"/demo","agent_id":"id","alias":null})
        };
        assert!(
            parse_client_message(
                &json!({"type":kind,"id":"r","message":message,"extra":true}).to_string()
            )
            .is_err()
        );
    }
}

#[test]
fn alias_and_record_id_validation_enforces_byte_limit_without_requiring_hex() {
    for value in [
        "".to_owned(),
        " \u{2003} ".into(),
        "bad\nname".into(),
        "bad\u{0085}name".into(),
        "a".repeat(513),
        "中".repeat(171),
    ] {
        assert!(parse("workspace.rename", json!({"address":"/demo","alias":value})).is_err());
        assert!(
            parse(
                "agent.rename",
                json!({"workspace":"/demo","agent_id":"id","alias":value})
            )
            .is_err()
        );
        assert!(
            parse(
                "agent.rename",
                json!({"workspace":"/demo","agent_id":value,"alias":null})
            )
            .is_err()
        );
    }
    for target in [
        "demo",
        "workspace:demo",
        "/demo//bad",
        "/demo/../bad",
        "/demo\\bad",
    ] {
        assert!(parse("workspace.rename", json!({"address":target,"alias":"ok"})).is_err());
        assert!(
            parse(
                "agent.rename",
                json!({"workspace":target,"agent_id":"id","alias":null})
            )
            .is_err()
        );
    }
}

#[test]
fn alias_results_roundtrip_and_only_allow_original_id_success_or_error() {
    for kind in ["workspace.rename.result", "agent.rename.result"] {
        for success in [true, false] {
            let mut raw = json!({"type":kind,"id":"rename-request","success":success});
            if !success {
                raw["error"] = json!({"code":"AgentRecordMissing","message":"not found"});
            }
            let result: ServerMessage = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(result.id(), Some("rename-request"));
            assert_eq!(serde_json::to_value(&result).unwrap(), raw);
            assert_eq!(
                serde_json::from_str::<ServerMessage>(&encode_server_message(&result).unwrap())
                    .unwrap(),
                result
            );
            for field in [
                "alias",
                "address",
                "workspace",
                "agent_id",
                "data",
                "payload",
                "message",
            ] {
                let mut invalid = raw.clone();
                invalid[field] = json!("unexpected");
                assert!(
                    serde_json::from_value::<ServerMessage>(invalid).is_err(),
                    "{kind}: {field}"
                );
            }
        }
        for raw in [
            json!({"type":kind,"success":true}),
            json!({"type":kind,"id":"","success":true}),
            json!({"type":kind,"id":"r","success":false}),
            json!({"type":kind,"id":"r","success":true,"error":null}),
            json!({"type":kind,"id":"r","success":true,"error":{"code":"Bad","message":"bad"}}),
            json!({"type":kind,"id":"r","success":false,"error":{"code":"Bad"}}),
            json!({"type":kind,"id":"r","success":false,"error":{"code":"Bad","message":"bad","extra":1}}),
        ] {
            assert!(serde_json::from_value::<ServerMessage>(raw).is_err());
        }
    }
    for result in [
        ServerMessage::WorkspaceRenameResult {
            id: "r".into(),
            success: true,
            error: Some(ProtocolErrorDto::new("Bad", "bad")),
        },
        ServerMessage::AgentRenameResult {
            id: "r".into(),
            success: false,
            error: None,
        },
    ] {
        assert!(encode_server_message(&result).is_err());
    }
}
