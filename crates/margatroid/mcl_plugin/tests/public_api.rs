use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use core_plugin::World;
use margatroid_types::{Message, ResourceId, ToolCall};
use mcl_plugin::{
    compile_mcl, AttachAgentMclRequest, MclCapabilityOwner, MclCompileRequest, MclEffect,
    MclProgramKind, MclRuntimeEvent, MclSource, WorldMclExt,
};

const STANDARD: &str = r#"
base context standard {
    block conversation: context persistent ordered by sequence;

    view messages: messages {
        select entry from conversation order by sequence asc;
    }

    view tools: tools {
        select resource from capabilities.dynamic order by resource.id asc;
    }

    request inference {
        system = agent.system;
        messages = messages;
        tools = tools;
        parameters = agent.model.parameters;
    }

    on agent.created transaction {
        restore capabilities.dynamic from capabilities.default;
    }

    on message.user as event transaction {
        append event.entry into conversation;
        emit inference using inference;
    }

    on message.assistant as event where event.tool_calls is empty transaction {
        append event.entry into conversation;
        finish turn;
    }
}
"#;

fn compile_standard() -> std::sync::Arc<mcl_plugin::MclProgram> {
    compile_mcl(MclCompileRequest {
        root: MclSource::new(
            ResourceId::parse("mcl:local/standard:latest").unwrap(),
            STANDARD,
            PathBuf::from("/image/mcl/local/standard/latest/main.mcl"),
        ),
        dependencies: BTreeMap::new(),
    })
    .unwrap()
}

#[test]
fn compiles_a_base_program() {
    let program = compile_standard();

    assert_eq!(program.kind(), MclProgramKind::Base);
    assert_eq!(program.blocks().len(), 1);
    assert_eq!(program.views().len(), 2);
    assert_eq!(program.requests().len(), 1);
    assert_eq!(program.handlers().len(), 3);
    assert_eq!(program.source_hash().as_str().len(), 64);
}

#[test]
fn executes_handlers_and_assembles_a_snapshot() {
    let mut world = World::new();
    let agent = world.spawn();
    let shell = ResourceId::parse("shell:local/sh:latest").unwrap();
    let initial_effects = world
        .attach_agent_mcl(
            agent,
            AttachAgentMclRequest {
                base: compile_standard(),
                system_prompt: "system".into(),
                context_window_tokens: 1_000_000,
                restored_messages: Vec::new(),
                default_visibility: BTreeSet::from([shell.clone()]),
            },
        )
        .unwrap();
    assert!(matches!(
        initial_effects.as_slice(),
        [MclEffect::ResolveResources { .. }]
    ));
    world
        .grant_agent_resource(agent, MclCapabilityOwner::Base, shell.clone())
        .unwrap();

    let effects = world
        .execute_mcl_event(
            agent,
            MclRuntimeEvent::UserMessage {
                entry: Message::User {
                    content: "hello".into(),
                },
            },
        )
        .unwrap();
    assert_eq!(
        effects,
        [MclEffect::RequestInference {
            request: "inference".into()
        }]
    );

    let snapshot = world.assemble_model_request(agent, "inference").unwrap();
    assert_eq!(snapshot.system, "system");
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.visible_resources, [shell]);
}

#[test]
fn capability_grants_are_isolated_by_owner() {
    let mut world = World::new();
    let agent = world.spawn();
    let resource = ResourceId::parse("tool:local/read:latest").unwrap();
    world
        .attach_agent_mcl(
            agent,
            AttachAgentMclRequest {
                base: compile_standard(),
                system_prompt: String::new(),
                context_window_tokens: 1_000_000,
                restored_messages: Vec::new(),
                default_visibility: BTreeSet::new(),
            },
        )
        .unwrap();
    let first = MclCapabilityOwner::External("first".into());
    let second = MclCapabilityOwner::External("second".into());
    world
        .grant_agent_resource(agent, first.clone(), resource.clone())
        .unwrap();
    world
        .grant_agent_resource(agent, second, resource.clone())
        .unwrap();
    world
        .revoke_agent_resource(agent, &first, &resource)
        .unwrap();

    assert_eq!(
        world
            .assemble_model_request(agent, "inference")
            .unwrap()
            .visible_resources,
        [resource]
    );
}

#[test]
fn compiles_the_complete_static_import_graph_once() {
    let common = ResourceId::parse("mcl:local/common:latest").unwrap();
    let history = ResourceId::parse("mcl:local/history:latest").unwrap();
    let program = compile_mcl(MclCompileRequest {
        root: MclSource::new(
            ResourceId::parse("mcl:local/root:latest").unwrap(),
            r#"import mcl:local/history:latest;
base context root {
    view tools: tools { select resource from capabilities.dynamic; }
    request inference { system = agent.system; messages = messages; tools = tools; }
}"#,
            PathBuf::from("/root/main.mcl"),
        ),
        dependencies: BTreeMap::from([
            (
                history.clone(),
                MclSource::new(
                    history.clone(),
                    r#"import mcl:local/common:latest;
module history { export view messages: messages { select entry from conversation; } }"#,
                    PathBuf::from("/history/main.mcl"),
                ),
            ),
            (
                common.clone(),
                MclSource::new(
                    common.clone(),
                    "module common { export block conversation: context persistent; }",
                    PathBuf::from("/common/main.mcl"),
                ),
            ),
        ]),
    })
    .unwrap();

    assert_eq!(
        program
            .imports()
            .iter()
            .map(|dependency| dependency.resource_id.clone())
            .collect::<Vec<_>>(),
        [common, history]
    );
    assert_eq!(program.blocks().len(), 1);
    assert_eq!(program.views().len(), 2);
}

#[test]
fn rejects_cycles_in_programmatic_compile_requests() {
    let first = ResourceId::parse("mcl:local/first:latest").unwrap();
    let second = ResourceId::parse("mcl:local/second:latest").unwrap();
    let error = compile_mcl(MclCompileRequest {
        root: MclSource::new(
            ResourceId::parse("mcl:local/root:latest").unwrap(),
            "import mcl:local/first:latest; base context root {}",
            PathBuf::from("/root/main.mcl"),
        ),
        dependencies: BTreeMap::from([
            (
                first.clone(),
                MclSource::new(
                    first,
                    "import mcl:local/second:latest; module first {}",
                    PathBuf::from("/first/main.mcl"),
                ),
            ),
            (
                second.clone(),
                MclSource::new(
                    second,
                    "import mcl:local/first:latest; module second {}",
                    PathBuf::from("/second/main.mcl"),
                ),
            ),
        ]),
    })
    .unwrap_err();

    assert_eq!(error.kind(), mcl_plugin::MclErrorKind::ImportCycle);
}

#[test]
fn tool_exchanges_close_atomically_and_expand_in_call_order() {
    let program = compile_mcl(MclCompileRequest {
        root: MclSource::new(
            ResourceId::parse("mcl:local/tools:latest").unwrap(),
            r#"base context tools {
block conversation: context persistent;
view messages: messages { select entry from conversation; }
view tools: tools { select resource from capabilities.dynamic; }
request inference { system = agent.system; messages = messages; tools = tools; }
on message.assistant where event.tool_calls is not empty {
    append event.exchange into conversation;
    emit tools event.tool_calls;
}
on message.tool { append event.entry into event.exchange; }
}"#,
            PathBuf::from("/tools/main.mcl"),
        ),
        dependencies: BTreeMap::new(),
    })
    .unwrap();
    let mut world = World::new();
    let agent = world.spawn();
    world
        .attach_agent_mcl(
            agent,
            AttachAgentMclRequest {
                base: program,
                system_prompt: String::new(),
                context_window_tokens: 1_000_000,
                restored_messages: Vec::new(),
                default_visibility: BTreeSet::new(),
            },
        )
        .unwrap();
    let calls = vec![
        ToolCall {
            id: "first".into(),
            tool_name: "tool0_first".into(),
            arguments: "{}".into(),
        },
        ToolCall {
            id: "second".into(),
            tool_name: "tool1_second".into(),
            arguments: "{}".into(),
        },
    ];
    world
        .execute_mcl_event(
            agent,
            MclRuntimeEvent::AssistantMessage {
                entry: Message::Assistant {
                    reasoning: None,
                    content: None,
                    tool_calls: calls,
                },
            },
        )
        .unwrap();
    world
        .execute_mcl_event(
            agent,
            MclRuntimeEvent::ToolMessage {
                entry: Message::Tool {
                    resource_id: ResourceId::parse("tool:local/second:latest").unwrap(),
                    tool_call_id: "second".into(),
                    content: "second response".into(),
                },
            },
        )
        .unwrap();
    assert_eq!(
        world
            .assemble_model_request(agent, "inference")
            .unwrap_err()
            .kind(),
        mcl_plugin::MclErrorKind::InvalidMessageSequence
    );
    world
        .execute_mcl_event(
            agent,
            MclRuntimeEvent::ToolMessage {
                entry: Message::Tool {
                    resource_id: ResourceId::parse("tool:local/first:latest").unwrap(),
                    tool_call_id: "first".into(),
                    content: "first response".into(),
                },
            },
        )
        .unwrap();

    let snapshot = world.assemble_model_request(agent, "inference").unwrap();
    assert_eq!(snapshot.messages.len(), 3);
    assert!(matches!(
        &snapshot.messages[1],
        Message::Tool { tool_call_id, .. } if tool_call_id == "first"
    ));
    assert!(matches!(
        &snapshot.messages[2],
        Message::Tool { tool_call_id, .. } if tool_call_id == "second"
    ));
}
