use super::*;

fn accumulated(calls: &[(&str, &str, &str)]) -> ResponseAccumulator {
    let mut accumulator = ResponseAccumulator::default();
    for (index, (id, name, arguments)) in calls.iter().enumerate() {
        accumulator
            .accept(ProviderFragment::ToolCallDelta {
                index,
                id: Some((*id).to_owned()),
                name: Some((*name).to_owned()),
                arguments: (*arguments).to_owned(),
            })
            .unwrap();
    }
    accumulator
        .accept(ProviderFragment::Finished(InferenceStopReason::ToolCalls))
        .unwrap();
    accumulator
}

#[test]
fn rejected_tool_calls_identify_the_exact_validation_failure() {
    for (calls, expected) in [
        (vec![("", "bash_exec", "{}")], "tool call has empty id"),
        (vec![("call", "", "{}")], "tool call has empty name"),
        (
            vec![("same", "bash_exec", "{}"), ("same", "bash_exec", "{}")],
            "duplicate tool call id=\"same\"",
        ),
        (
            vec![("call", "bash_exec", "[]")],
            "tool call arguments must be an object",
        ),
        (
            vec![("call", "bash_exec", "{")],
            "incomplete tool arguments",
        ),
    ] {
        let failure = accumulated(&calls).response().unwrap_err();
        assert_eq!(failure.kind, InferenceErrorKind::ResponseIncomplete);
        assert_eq!(failure.message, expected);
    }
    let accepted = accumulated(&[("call", "rea_binary_session", "{}")])
        .response()
        .unwrap();
    assert_eq!(accepted.tool_calls[0].name, "rea_binary_session");
}

#[test]
fn call_diagnostics_are_bounded_escaped_and_exclude_argument_content() {
    let raw = "\n\r\t\"\\中".repeat(100);
    let rendered = tool_call_diagnostic(&raw);
    assert!(rendered.len() <= 128);
    assert!(!rendered.contains('\n'));
    assert!(rendered.contains("\\n"));
    assert!(rendered.ends_with("...\""));
    let failure = accumulated(&[
        ("call", "any_name", r#"{"private":"secret-argument"}"#),
        ("call", "any_name", "{}"),
    ])
    .response()
    .unwrap_err();
    assert!(!failure.message.contains("secret-argument"));
}
