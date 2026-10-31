use mcl_core::*;
use serde_json::json;
fn parse(text: &str, arguments: Vec<serde_json::Value>) -> Result<MclOperation, MclError> {
    parse_instruction(&MclInstruction {
        text: text.into(),
        arguments,
    })
}
#[test]
fn config_has_its_own_namespace_and_scalar_parameters() {
    assert_eq!(
        parse("CONFIG GET", vec![]).unwrap(),
        MclOperation::Config(ConfigOperation::Get)
    );
    let MclOperation::Config(ConfigOperation::Set { path, value }) = parse(
        "CONFIG SET ? ?",
        vec![json!("compaction.threshold"), json!(0.8)],
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(path, "compaction.threshold");
    assert_eq!(value, toml::Value::Float(0.8));
    assert!(parse("CONFIG SET ? ?", vec![json!("x"), json!(null)]).is_err());
    assert!(parse("GET CONFIG", vec![]).is_ok());
    assert!(parse("CONFIG DELETE ?", vec![json!("a..b")]).is_err());
}
#[test]
fn message_only_fields_and_effect_arity_are_explicit() {
    assert!(matches!(
        parse(
            "CREATE TEMP BLOCK msg (instructions, compact_context)",
            vec![]
        )
        .unwrap(),
        MclOperation::Create {
            temporary: true,
            ..
        }
    ));
    assert!(parse("CREATE BLOCK b (messages MESSAGE)", vec![]).is_err());
    assert!(parse("CREATE BLOCK b (a, a)", vec![]).is_err());
    assert!(parse("EMIT EFFECT ready", vec![]).is_err());
    assert!(parse("EFFECT ready", vec![]).is_ok());
    assert!(parse(
        "EFFECT inference (msg.instructions, session.conversation) ?",
        vec![json!(["bash"])]
    )
    .is_ok());
    assert!(parse("EFFECT inference ?", vec![json!([])]).is_err());
}
#[test]
fn selectors_keep_range_commas_and_reject_mixed_sign() {
    let MclOperation::Get(value) =
        parse("GET soul, b.messages[0,2], b.messages[-1]", vec![]).unwrap()
    else {
        panic!()
    };
    assert_eq!(value.sources.len(), 3);
    assert_eq!(
        parse_selector("b.messages[-3,-1]").unwrap().range,
        Selection::Range(-3, -1)
    );
    assert!(parse_selector("b.messages[-1,2]").is_err());
    assert!(parse("INJECT [] TO b.messages[0,1]", vec![]).is_ok());
    assert!(parse("INJECT ? TO b.messages", vec![]).is_err());
}
#[test]
fn values_preserve_driver_result_shape() {
    assert_eq!(
        serde_json::to_value(MclValue::Array(MessageArray::Message(vec![]))).unwrap(),
        json!({"Array":{"Message":[]}})
    );
}
