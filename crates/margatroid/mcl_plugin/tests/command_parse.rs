use mcl_plugin::{parse_operation, MclEffectCommand, MclOperation};
use serde_json::json;

#[test]
fn parses_base_lua_non_effect_commands_without_consuming_the_binding_wrongly() {
    let operation = parse_operation(
        "INJECT ? TO recent_conversation FROM msg",
        Some(&json!({
            "type": "user",
            "content": "hello"
        })),
    )
    .unwrap();
    assert!(matches!(operation, MclOperation::Inject { .. }));

    assert!(matches!(
        parse_operation("INJECT soul TO system_prompt FROM msg", None).unwrap(),
        MclOperation::Inject { .. }
    ));

    let operation = parse_operation(
        "INJECT SELECT recent_conversation FROM msg COVER history_conversation FROM msg",
        None,
    )
    .unwrap();
    assert!(matches!(operation, MclOperation::CoverInner { .. }));

    let operation = parse_operation("DELETE recent_conversation FIRST FROM msg", None).unwrap();
    assert!(matches!(operation, MclOperation::DeleteFirst { .. }));

    let operation = parse_operation(
        "DELETE pending_tool FROM msg WHERE id == ?",
        Some(&json!("call-1")),
    )
    .unwrap();
    assert!(matches!(operation, MclOperation::DeleteWhere { .. }));
}

#[test]
fn parses_multi_value_inject_with_comma_separated_aliases() {
    let operation =
        parse_operation("INJECT review, list_dir TO tool_default FROM tool", None).unwrap();
    let MclOperation::InjectMany { values, .. } = operation else {
        panic!("expected InjectMany")
    };
    assert_eq!(values.len(), 2);
    assert!(parse_operation(
        "INJECT review, list_dir TO tool_default FROM tool",
        Some(&json!(1))
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
fn parses_visibility_source_effect() {
    let operation = parse_operation(
        "EMIT EFFECT visibility_source (SELECT tool_dynamic FROM tool)",
        None,
    )
    .unwrap();
    assert!(matches!(
        operation,
        MclOperation::Emit {
            effect: MclEffectCommand::VisibilitySource { .. }
        }
    ));
    assert!(parse_operation(
        "EMIT EFFECT visibility_source (SELECT tool_dynamic FROM tool)",
        Some(&json!(1))
    )
    .is_err());
}

#[test]
fn parses_default_visibility_source() {
    assert!(matches!(
        parse_operation(
            "EMIT EFFECT default_visibility_source (SELECT tool_default FROM tool)",
            None,
        )
        .unwrap(),
        MclOperation::Emit {
            effect: MclEffectCommand::DefaultVisibilitySource { .. }
        }
    ));
    assert!(parse_operation("EMIT EFFECT visibility_reset", None).is_err());
}

#[test]
fn parses_delete_where_for_resource_ids() {
    assert!(matches!(
        parse_operation(
            "DELETE tool_dynamic FROM tool WHERE id == ?",
            Some(&json!("tool:local/read-file:latest")),
        )
        .unwrap(),
        MclOperation::DeleteWhere { .. }
    ));
}

#[test]
fn rejects_unconsumed_bindings_and_second_commands() {
    assert!(parse_operation("SELECT messages FROM msg", Some(&json!(1))).is_err());
    assert!(parse_operation("DELETE messages FROM msg; SELECT messages FROM msg", None).is_err());
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
        "CREATE BLOCK msg ( system_prompt MESSAGE, history MESSAGE, MERGE system_prompt, history FROM msg AS merged, )",
        None,
    )
    .unwrap();
    let MclOperation::CreateBlock { fields, .. } = operation else {
        panic!("expected CREATE BLOCK")
    };
    assert_eq!(fields.len(), 3);
}
