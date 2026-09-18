use mcl_plugin::{parse_operation, MclEffectCommand, MclInjectSource, MclOperation, MclSelector};
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
            selector: MclSelector::Index { index: -1, .. }
        }
    ));
    assert!(matches!(
        parse_operation("GET req.ctx[0, 4]", None).unwrap(),
        MclOperation::Get {
            selector: MclSelector::Range {
                start: 0,
                end: 4,
                ..
            }
        }
    ));
    assert!(parse_operation("GET req.ctx[0,-1]", None).is_err());
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
            source: MclInjectSource::Selector(MclSelector::Range { .. }),
            target: MclSelector::Index { index: -1, .. }
        }
    ));
    assert!(matches!(
        parse_operation("INJECT req.ctx TO msg.recent_conversation[0,4]", None).unwrap(),
        MclOperation::Inject {
            target: MclSelector::Range { .. },
            ..
        }
    ));
}

#[test]
fn rejects_old_select_delete_and_tool_call_block_syntax() {
    assert!(parse_operation("SELECT messages FROM msg", None).is_err());
    assert!(parse_operation("DELETE messages FROM msg", None).is_err());
    assert!(parse_operation(
        "CREATE BLOCK msg ( pending TOOL_CALL, )",
        None
    )
    .is_err());
}

#[test]
fn parses_flat_assistant_message_for_history_append() {
    let operation = parse_operation(
        "EMIT EFFECT history_append ?",
        Some(&json!({
            "type": "assistant",
            "reasoning": null,
            "content": "hello",
            "tool_calls": [],
            "usage": {"input_tokens": 1, "output_tokens": 2, "cache_hit_tokens": 3}
        })),
    )
    .unwrap();
    assert!(matches!(
        operation,
        MclOperation::Emit {
            effect: MclEffectCommand::HistoryAppend { .. }
        }
    ));
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
fn parses_effect_parameter_shapes() {
    assert!(matches!(
        parse_operation("EMIT EFFECT start", None).unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::Start
        }
    ));
    assert!(parse_operation("EMIT EFFECT start (req)", None).is_err());
    assert!(matches!(
        parse_operation("EMIT EFFECT inference (req)", None).unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::Inference { .. }
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
