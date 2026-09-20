use mcl_plugin::{
    parse_operation, MclEffectCommand, MclEndpoint, MclInjectSource, MclOperation, MclRange,
    MclSelector, MclSelectorPosition,
};
use serde_json::json;

#[test]
fn parses_get_selectors() {
    assert!(matches!(
        parse_operation("GET req.ctx", None).unwrap(),
        MclOperation::Get {
            selector: MclSelector::All(_)
        }
    ));
    assert!(matches!(
        parse_operation("GET req.ctx[-1]", None).unwrap(),
        MclOperation::Get {
            selector: MclSelector::Raw {
                range: MclRange::Single(-1),
                position: MclSelectorPosition::Get,
                ..
            }
        }
    ));
    assert!(matches!(
        parse_operation("GET req.ctx[0, 4]", None).unwrap(),
        MclOperation::Get {
            selector: MclSelector::Raw {
                range: MclRange::Range(MclEndpoint::Value(0), MclEndpoint::Value(4)),
                ..
            }
        }
    ));
    assert!(parse_operation("GET req.ctx[0,-1]", None).is_ok());
}

#[test]
fn parses_unified_inject_sources_and_targets() {
    assert!(matches!(
        parse_operation("INJECT [] TO msg.recent_conversation", None).unwrap(),
        MclOperation::Inject {
            source: MclInjectSource::Bindings(_),
            target: MclSelector::All(_)
        }
    ));
    assert!(matches!(
        parse_operation("INJECT ? TO msg.recent_conversation", Some(&json!(null))).unwrap(),
        MclOperation::Inject {
            source: MclInjectSource::Bindings(_),
            target: MclSelector::All(_)
        }
    ));
    assert!(matches!(
        parse_operation("INJECT req.ctx[0,4] TO msg.recent_conversation[-1]", None).unwrap(),
        MclOperation::Inject {
            source: MclInjectSource::Selector(MclSelector::Raw {
                range: MclRange::Range(..),
                position: MclSelectorPosition::Get,
                ..
            }),
            target: MclSelector::Raw {
                range: MclRange::Single(-1),
                position: MclSelectorPosition::Inject,
                ..
            }
        }
    ));
    assert!(matches!(
        parse_operation("INJECT req.ctx TO msg.recent_conversation[0,4]", None).unwrap(),
        MclOperation::Inject {
            target: MclSelector::Raw {
                range: MclRange::Range(..),
                ..
            },
            ..
        }
    ));
}

#[test]
fn rejects_old_select_delete_and_tool_call_block_syntax() {
    assert!(parse_operation("SELECT messages FROM msg", None).is_err());
    assert!(parse_operation("DELETE messages FROM msg", None).is_err());
    assert!(parse_operation("CREATE BLOCK msg ( pending TOOL_CALL, )", None).is_err());
}

#[test]
fn parses_explicit_history_effects() {
    let message = json!({
        "type": "assistant",
        "reasoning": null,
        "content": "hello",
        "tool_calls": [],
        "usage": {"input_tokens": 1, "output_tokens": 2, "cache_hit_tokens": 3}
    });
    assert!(matches!(
        parse_operation("EMIT EFFECT history_append FROM ?", Some(&message)).unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::HistoryAppend { .. }
        }
    ));
    let record = json!({"kind": "mcl", "cmd": "GET req.ctx", "arg": null});
    assert!(matches!(
        parse_operation("EMIT EFFECT history_record FROM ?", Some(&record)).unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::HistoryRecord { .. }
        }
    ));
    assert!(parse_operation("EMIT EFFECT history_append ?", Some(&message)).is_err());
    assert!(parse_operation("EMIT EFFECT history_append FROM ?", Some(&record)).is_err());
    assert!(parse_operation("EMIT EFFECT history_record FROM ?", Some(&message)).is_err());
}

#[test]
fn parses_state_bind_and_load() {
    assert!(matches!(
        parse_operation("BIND setting TO STATE agent_setting", None).unwrap(),
        MclOperation::BindState { .. }
    ));
    assert!(matches!(
        parse_operation("LOAD STATE agent_setting INTO setting", None).unwrap(),
        MclOperation::LoadState { .. }
    ));
}

#[test]
fn parses_sandbox_use_arguments() {
    assert!(matches!(
        parse_operation(
            "EMIT EFFECT sandbox_use FROM ?",
            Some(&json!("workspace_write"))
        )
        .unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::SandboxUse { .. }
        }
    ));
    assert!(matches!(
        parse_operation(
            "EMIT EFFECT sandbox_use FROM ?",
            Some(&json!(["workspace_write", "read_only"]))
        )
        .unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::SandboxUse { .. }
        }
    ));
    assert!(matches!(
        parse_operation("EMIT EFFECT sandbox_use FROM ?", Some(&json!({}))).unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::SandboxUse { .. }
        }
    ));
    assert!(parse_operation(
        "EMIT EFFECT sandbox_use FROM ?",
        Some(&json!({"unexpected": "field"}))
    )
    .is_err());
    assert!(parse_operation("EMIT EFFECT sandbox_use FROM ?", Some(&json!(7))).is_err());
}

#[test]
fn parses_uniform_effect_arguments() {
    assert!(matches!(
        parse_operation("EMIT EFFECT start", None).unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::Start
        }
    ));
    assert!(matches!(
        parse_operation("EMIT EFFECT inference FROM req", None).unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::Inference { .. }
        }
    ));
    assert!(matches!(
        parse_operation("EMIT EFFECT visibility_source FROM tool.dynamic", None).unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::VisibilitySource { .. }
        }
    ));
    assert!(parse_operation("EMIT EFFECT start FROM req", None).is_err());
    assert!(parse_operation("EMIT EFFECT inference (req)", None).is_err());
    assert!(parse_operation("EMIT EFFECT tool_call ?", Some(&json!([]))).is_err());
}

#[test]
fn parses_multiline_driver_block() {
    let command = "CREATE BLOCK msg (\n        system_prompt MESSAGE,\n        compact_prompt MESSAGE,\n        compact_context MESSAGE,\n        history_conversation MESSAGE,\n        recent_conversation MESSAGE,\n    )";
    assert!(parse_operation(command, None).is_ok());
}

#[test]
fn parses_every_selector_form_into_its_lexical_range() {
    let range = |command: &str| match parse_operation(command, None).unwrap() {
        MclOperation::Get {
            selector: MclSelector::Raw { range, .. },
        } => range,
        other => panic!("expected a raw selector for {command}: {other:?}"),
    };
    assert_eq!(range("GET msg.a[2]"), MclRange::Single(2));
    assert_eq!(range("GET msg.a[-2]"), MclRange::Single(-2));
    assert_eq!(
        range("GET msg.a[1,]"),
        MclRange::Range(MclEndpoint::Value(1), MclEndpoint::Open)
    );
    assert_eq!(
        range("GET msg.a[,1]"),
        MclRange::Range(MclEndpoint::Open, MclEndpoint::Value(1))
    );
    assert_eq!(
        range("GET msg.a[1,2]"),
        MclRange::Range(MclEndpoint::Value(1), MclEndpoint::Value(2))
    );
    assert_eq!(range("GET msg.a[]"), MclRange::Empty);
    assert_eq!(
        range("GET msg.a[,]"),
        MclRange::Range(MclEndpoint::Open, MclEndpoint::Open)
    );
    assert_eq!(
        range("GET msg.a[1,-2]"),
        MclRange::Range(MclEndpoint::Value(1), MclEndpoint::Value(-2))
    );
}

#[test]
fn parses_inject_targets_with_the_inject_position() {
    let target = |command: &str| match parse_operation(command, Some(&json!({}))).unwrap() {
        MclOperation::Inject { target, .. } => target,
        other => panic!("expected an inject for {command}: {other:?}"),
    };
    assert!(matches!(
        target("INJECT ? TO msg.a[-1]"),
        MclSelector::Raw {
            range: MclRange::Single(-1),
            position: MclSelectorPosition::Inject,
            ..
        }
    ));
    assert!(matches!(
        target("INJECT ? TO msg.a[0,2]"),
        MclSelector::Raw {
            range: MclRange::Range(..),
            position: MclSelectorPosition::Inject,
            ..
        }
    ));
}

#[test]
fn parses_create_fields_and_discards_comma_tokens() {
    let operation = parse_operation(
        "CREATE BLOCK msg ( system_prompt MESSAGE history MESSAGE )",
        None,
    )
    .unwrap();
    let MclOperation::CreateBlock { fields, .. } = operation else {
        panic!("expected CREATE BLOCK")
    };
    assert_eq!(fields.len(), 2);
}
