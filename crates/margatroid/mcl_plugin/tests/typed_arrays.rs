use margatroid_types::block_types::{AgentBlock, BlockInner, BlockPath, InnerType};
use margatroid_types::message_types::{AgentMessage, ToolCall};
use mcl_plugin::parser::*;
use mcl_plugin::MclError;
use serde_json::json;

fn path() -> BlockPath {
    BlockPath {
        block_id: "b".into(),
        inner_id: "m".into(),
    }
}
fn blocks() -> AgentBlock {
    let mut blocks = AgentBlock::default();
    blocks
        .create(
            "b".into(),
            vec![
                ("m".into(), InnerType::Message),
                ("t".into(), InnerType::Tool),
            ],
        )
        .unwrap();
    blocks
        .replace(
            &path(),
            0,
            0,
            BlockInner::Message(
                (0..4)
                    .map(|i| AgentMessage::User {
                        content: i.to_string(),
                    })
                    .collect(),
            ),
        )
        .unwrap();
    blocks
}
fn resolve(alias: &str) -> Result<BlockInner, MclError> {
    match alias {
        "soul" => Ok(BlockInner::Message(vec![AgentMessage::System {
            content: "system".into(),
        }])),
        "empty-tool" => Ok(BlockInner::Tool(vec![])),
        _ => Err(MclError::AliasMissing),
    }
}
#[test]
fn explicit_import_kind_alias_and_removed_contracts() {
    for text in [
        "IMPORT TOOL tool:local/bash:latest AS bash",
        "IMPORT MESSAGE prompt:local/soul AS soul",
        "IMPORT SANDBOX sandbox:local/write AS workspace_write",
    ] {
        assert!(parse(text, &[]).is_ok(), "{text}");
    }
    for text in [
        "IMPORT TOOL skill:local/review AS review",
        "IMPORT TOOL hook:local/run AS review-flow",
        "IMPORT tool:local/bash AS bash",
        "IMPORT TOOL prompt:local/soul AS soul",
        "IMPORT MESSAGE tool:local/bash AS bash",
        "IMPORT TOOL tool:local/bash AS b.a",
        "IMPORT TOOL tool:local/bash AS -bash",
        "CREATE BLOCK x (r RESOURCE)",
        "CREATE BLOCK x (t TOOL_CALL)",
        "GET tool:local/bash",
        "GET soul,",
        "GET ,soul",
        "GET soul[]",
    ] {
        assert!(parse(text, &[]).is_err(), "{text}");
    }
}
#[test]
fn aliases_selectors_snapshot_and_mixed_empty() {
    let blocks = blocks();
    let expression = parse_array_expression("soul, b.m[0,2], soul, b.m[-1]").unwrap();
    let value = select_array_with(&blocks, &expression, &resolve).unwrap();
    assert_eq!(value.len(), 5);
    let BlockInner::Message(messages) = value else {
        panic!()
    };
    assert!(matches!(&messages[0], AgentMessage::System { .. }));
    assert_eq!(messages[0], messages[3]);
    assert_eq!(
        select_array_with(
            &blocks,
            &parse_array_expression("b.m[], empty-tool").unwrap(),
            &resolve
        ),
        Err(MclError::TypeMismatch)
    );
    assert_eq!(
        select_array_with(
            &blocks,
            &parse_array_expression("unknown").unwrap(),
            &resolve
        ),
        Err(MclError::AliasMissing)
    );
    let target = parse_selector("b.m[1,3]", SelectorPosition::Write).unwrap();
    let source = ArrayParameter::Expression(parse_array_expression("soul, b.m[-1]").unwrap());
    let prepared = prepare_inject_with(&resolve, &blocks, &source, &target, &[]).unwrap();
    assert_eq!(prepared.replacement.len(), 2);
    assert_eq!(blocks.get(&path()).unwrap().len(), 4);
}
#[test]
fn checked_ranges_and_typed_empty() {
    let blocks = blocks();
    for (text, start, end) in [("b.m[-1]", 3, 4), ("b.m[-3,-1]", 2, 4), ("b.m[]", 0, 0)] {
        let mapped =
            map_selector(&parse_selector(text, SelectorPosition::Read).unwrap(), 4).unwrap();
        assert_eq!((mapped.start, mapped.end), (start, end));
    }
    for text in [
        "b.m[,]",
        "b.m[-2,2]",
        "b.m[3,1]",
        "b.m[4]",
        "b.m[-5]",
        "b.m[9223372036854775807]",
        "b.m[-9223372036854775808]",
    ] {
        assert!(map_selector(&parse_selector(text, SelectorPosition::Read).unwrap(), 4).is_err());
    }
    assert_eq!(
        select_array(&blocks, &parse_array_expression("b.t[0]").unwrap()).unwrap(),
        BlockInner::Tool(vec![])
    );
}
#[test]
fn tool_calls_are_extracted_only_from_one_assistant() {
    let assistant = AgentMessage::Assistant {
        reasoning: None,
        content: None,
        tool_calls: vec![],
        usage: None,
    };
    assert!(validate_tool_parameters(&[
        BlockInner::Message(vec![assistant.clone()]),
        BlockInner::Tool(vec![])
    ])
    .is_ok());
    for messages in [
        vec![],
        vec![AgentMessage::User {
            content: "no".into(),
        }],
        vec![assistant.clone(), assistant],
    ] {
        assert_eq!(
            validate_tool_parameters(&[BlockInner::Message(messages), BlockInner::Tool(vec![])]),
            Err(MclError::ToolCallInvalid)
        );
    }
    let duplicate = ToolCall {
        id: "c".into(),
        tool_name: "bash".into(),
        arguments: "{}".into(),
    };
    let message = AgentMessage::Assistant {
        reasoning: None,
        content: None,
        tool_calls: vec![duplicate.clone(), duplicate],
        usage: None,
    };
    assert_eq!(
        validate_tool_parameters(&[BlockInner::Message(vec![message]), BlockInner::Tool(vec![])]),
        Err(MclError::ToolCallInvalid)
    );
    assert!(decode_manual_array(&json!(["tool:local/bash"]), InnerType::Tool).is_err());
    assert!(decode_manual_array(
        &json!([{"type":"system","content":"yes"}]),
        InnerType::Message
    )
    .is_ok());
}
