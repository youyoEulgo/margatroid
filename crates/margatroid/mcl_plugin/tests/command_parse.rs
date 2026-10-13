use mcl_plugin::parser::{
    parse, ArrayParameter, ArraySource, ParsedOperation, Range, SelectorPosition,
};
use serde_json::json;

#[test]
fn selector_lists_and_manual_arguments_use_new_contract() {
    let ParsedOperation::Get { expression } = parse("GET req.ctx[-1], req.ctx[0, 4]", &[]).unwrap()
    else {
        panic!()
    };
    assert_eq!(expression.sources.len(), 2);
    let ArraySource::Selector(selector) = &expression.sources[0] else {
        panic!()
    };
    assert_eq!(selector.range, Range::Single(-1));
    let ParsedOperation::Inject { source, target } =
        parse("INJECT ? TO msg.ctx[-1]", &[json!([])]).unwrap()
    else {
        panic!()
    };
    assert_eq!(source, ArrayParameter::Manual { index: 0 });
    assert_eq!(target.position, SelectorPosition::Write);
    assert!(parse("INJECT ? TO msg.ctx", &[json!({})]).is_err());
}

#[test]
fn old_business_syntax_is_not_accepted() {
    for text in [
        "MERGE ctx FROM req",
        "CREATE REF_BLOCK req ()",
        "BIND setting TO STATE s",
        "LOAD STATE s INTO setting",
        "EXPOSE req",
        "EMIT EFFECT history_record FROM ?",
        "EMIT EFFECT visibility_source FROM req",
        "EMIT EFFECT inference FROM req",
        "SELECT ctx FROM req",
        "DELETE ctx FROM req",
    ] {
        assert!(parse(text, &[]).is_err(), "{text}");
    }
}

#[test]
fn create_and_effect_inputs_are_three_typed_arrays() {
    assert!(parse(
        "CREATE BLOCK msg (messages MESSAGE, resources TOOL, sandboxes SANDBOX)",
        &[]
    )
    .is_ok());
    assert!(parse("CREATE BLOCK msg (messages MESSAGE) trailing", &[]).is_err());
    assert!(parse("CREATE BLOCK msg (messages MESSAGE, messages TOOL)", &[]).is_err());
    assert!(parse("EMIT EFFECT inference ? ?", &[json!([]), json!([])]).is_ok());
    assert!(parse("EMIT EFFECT tool_call (msg.resources) (msg.calls)", &[]).is_ok());
    assert!(parse(
        "EMIT EFFECT history_append ?",
        &[json!([{"type":"user","content":"hello"}])]
    )
    .is_ok());
    assert!(parse("EMIT EFFECT inference ? ?", &[json!([])]).is_err());
    for (text, expected) in [
        ("CREATE BLOCK b (m MESSAGE)", false),
        ("CREATE TEMP BLOCK b (m MESSAGE)", true),
    ] {
        let ParsedOperation::Create { temporary, .. } = parse(text, &[]).unwrap() else {
            panic!()
        };
        assert_eq!(temporary, expected);
    }
    assert!(parse("EMIT EFFECT finish", &[]).is_err());
    assert!(parse("EMIT EFFECT finish_turn", &[]).is_ok());
}
