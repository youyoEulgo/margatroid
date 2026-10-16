use margatroid_protocol::*;
use margatroid_types::{
    history_components::HistoryError,
    mcl_events::{MclCommand, MclCommandResult, MclValue},
    message_types::{AgentMessage, InferenceResponse, StopReason, TokenUsage},
};
use resource_id_plugin::ResourceID;
fn id(kind: &str, name: &str) -> ResourceID {
    ResourceID::new(kind, "test", name, None).unwrap()
}
#[test]
fn workspace_member_failures_are_backward_compatible_and_lossless() {
    let legacy = serde_json::json!({
        "id": "workspace:test/demo:latest", "name": "demo", "project_root": "/tmp/demo",
        "manager": "agent:test/manager:latest", "agents": ["agent:test/manager:latest"]
    });
    let mut dto: WorkspaceInfoDto = serde_json::from_value(legacy.clone()).unwrap();
    assert!(dto.member_failures.is_empty());
    assert_eq!(serde_json::to_value(&dto).unwrap(), legacy);
    dto.member_failures.push(WorkspaceMemberFailureDto {
        agent: ResourceIdDto("agent:test/manager:latest".into()),
        driver: ResourceIdDto("agent-driver:test/broken:latest".into()),
        error: "AgentStartFailed:原始错误".into(),
    });
    let encoded = serde_json::to_value(&dto).unwrap();
    assert_eq!(
        encoded["member_failures"][0]["error"],
        "AgentStartFailed:原始错误"
    );
    assert_eq!(
        serde_json::from_value::<WorkspaceInfoDto>(encoded).unwrap(),
        dto
    );
}

#[test]
fn workspace_entity_and_backend_project_failures_but_definition_does_not() {
    use core_plugin::App;
    use margatroid_types::{
        agent_components::Agent,
        workspace_components::{
            Workspace, WorkspaceAgentDefinition, WorkspaceDefinition, WorkspaceError,
            WorkspaceErrorKind, WorkspaceMemberFailure, WorkspaceState,
        },
        workspace_execution::spawn_workspace_execution,
    };
    use resource_id_plugin::WorldResourceExt;
    let mut app = App::new();
    app.add_plugin(resource_id_plugin::ResourcePlugin);
    let world = app.world_mut();
    world.register_library::<Workspace>("workspace").unwrap();
    world.register_library::<Agent>("agent").unwrap();
    let definition = WorkspaceDefinition {
        id: id("workspace", "partial"),
        name: "partial".into(),
        manager: "manager".into(),
        project_root: "/tmp/partial".into(),
        agents: vec![WorkspaceAgentDefinition {
            id: id("agent", "manager"),
            name: "manager".into(),
            driver: id("agent-driver", "broken"),
            home_root: "/tmp/home".into(),
            history_path: "/tmp/history".into(),
        }],
    };
    let definition_dto: WorkspaceInfoDto = (&definition).into_dto(()).unwrap();
    assert!(definition_dto.member_failures.is_empty());
    let (entry, close) = spawn_workspace_execution().unwrap();
    let mut component = Workspace::new(definition, WorkspaceState::Running, entry, close.clone());
    let error = WorkspaceError::new(
        WorkspaceErrorKind::AgentDriverLoadFailed,
        "missing module 原始错误",
    );
    component.record_member_failure(
        id("agent", "manager"),
        WorkspaceMemberFailure {
            driver: id("agent-driver", "broken"),
            error: error.clone(),
        },
    );
    let entity = world.spawn();
    world.insert_library(entity, component).unwrap();
    let actual: WorkspaceInfoDto = entity.into_dto(&*world).unwrap();
    assert_eq!(
        actual.member_failures,
        vec![WorkspaceMemberFailureDto {
            agent: id("agent", "manager").into_dto(()).unwrap(),
            driver: id("agent-driver", "broken").into_dto(()).unwrap(),
            error: error.to_string(),
        }]
    );
    let backend: BackendStateDto = ().into_dto(&*world).unwrap();
    assert_eq!(backend.workspaces, vec![actual.clone()]);
    let missing = resolve_agent(world, &actual.reference(), None).unwrap_err();
    assert_eq!(missing.kind(), ProtocolErrorKind::AgentNotFound);
    assert!(missing.message().contains("manager unavailable"));
    assert!(missing.message().contains("missing module 原始错误"));
    let wrong = WorkspaceReferenceDto::new("wrong", "/tmp/wrong");
    assert_eq!(
        resolve_agent(world, &wrong, None).unwrap_err().kind(),
        ProtocolErrorKind::WorkspaceNotFound
    );
    tokio_runtime_close(close);
}

#[test]
fn history_projection_preserves_real_and_unknown_creation_times() {
    for created_at_ms in [0, 1_800_000_123_456] {
        let record = margatroid_types::history_components::HistoryRecord {
            created_at_ms,
            key: "history-key".into(),
            source: "test".into(),
            turn: None,
            kind: "message".into(),
            content: "hello".into(),
            payload: serde_json::json!({}),
        };
        let dto: HistoryMessageDto = (4usize, record).into_dto(()).unwrap();
        assert_eq!(dto.sequence, 5);
        assert_eq!(dto.created_at_ms, created_at_ms);
        assert_eq!(
            serde_json::to_value(dto).unwrap()["created_at_ms"],
            created_at_ms
        );
    }
}

#[test]
fn history_projection_preserves_split_and_legacy_message_rows_without_rewriting() {
    for (content, payload) in [
        ("hello", serde_json::json!({"type":"user"})),
        ("failed", serde_json::json!({"type":"error"})),
        (
            "",
            serde_json::json!({"type":"inject","messages":[{"type":"user","content":"nested"}]}),
        ),
        (
            "legacy serialized JSON",
            serde_json::json!({"type":"user","content":"old"}),
        ),
    ] {
        let record = margatroid_types::history_components::HistoryRecord {
            created_at_ms: 123,
            key: "key".into(),
            source: "test".into(),
            turn: Some("turn".into()),
            kind: "message".into(),
            content: content.into(),
            payload: payload.clone(),
        };
        let dto: HistoryMessageDto = (0usize, record).into_dto(()).unwrap();
        assert_eq!(dto.content, content);
        assert_eq!(dto.payload, payload);
        assert_eq!(dto.created_at_ms, 123);
    }
}

#[test]
fn nested_injection_preserves_all_shared_message_shapes() {
    let dto = AgentMessageDto::Inject {
        messages: vec![
            AgentMessageDto::Inject {
                messages: vec![
                    AgentMessageDto::ToolCall {
                        resource_id: None,
                        tool_call_id: "call".into(),
                        tool_name: "unknown".into(),
                        content: "failed".into(),
                        images: vec![],
                        failed: true,
                    },
                    AgentMessageDto::Assistant {
                        replay: None,
                        reasoning: Some("thought".into()),
                        content: None,
                        tool_calls: vec![],
                        stop_reason: None,
                        usage: Some(TokenUsage {
                            input_tokens: 9,
                            output_tokens: 1,
                            cache_hit_tokens: 2,
                        }),
                    },
                ],
            },
            AgentMessageDto::Error {
                message: "error".into(),
            },
            AgentMessageDto::System {
                content: "real system prompt".into(),
            },
        ],
    };
    let domain: AgentMessage = dto.clone().into_domain(()).unwrap();
    let roundtrip: AgentMessageDto = (&domain).into_dto(()).unwrap();
    assert_eq!(roundtrip, dto);
    assert_eq!(
        serde_json::to_value(&roundtrip).unwrap()["messages"][2]["type"],
        "system"
    );
    assert_eq!(
        serde_json::to_value(&roundtrip).unwrap()["messages"][0]["messages"][0]["type"],
        "tool_call"
    );
}
#[test]
fn opaque_assistant_replay_round_trips_through_messages_and_history() {
    use margatroid_types::history_components::{message_history_parts, HistoryRecord};

    let replay = serde_json::json!({
        "messages": {"signed_reasoning": [{"signature": "opaque+/=", "data": [1, null, false]}]},
        "responses": {"encrypted_content": "opaque-token", "unknown": {"future": true}}
    });
    let wire = serde_json::json!({
        "type": "assistant", "reasoning": "thought", "content": "answer",
        "tool_calls": [], "usage": null, "stop_reason": null, "replay": replay
    });
    let dto: AgentMessageDto = serde_json::from_value(wire.clone()).unwrap();
    let domain: AgentMessage = dto.into_domain(()).unwrap();
    let roundtrip: AgentMessageDto = (&domain).into_dto(()).unwrap();
    assert_eq!(serde_json::to_value(&roundtrip).unwrap(), wire);
    assert_eq!(roundtrip.into_domain(()).unwrap(), domain);

    let (content, payload) = message_history_parts(&domain);
    assert_eq!(payload["replay"], replay);
    let history: HistoryMessageDto = (
        0,
        HistoryRecord {
            created_at_ms: 123,
            key: "signed-message".into(),
            source: "test".into(),
            turn: None,
            kind: "message".into(),
            content,
            payload,
        },
    )
        .into_dto(())
        .unwrap();
    let history: HistoryMessageDto =
        serde_json::from_value(serde_json::to_value(history).unwrap()).unwrap();
    assert_eq!(history.payload["replay"], replay);
    let mut restored = history.payload;
    restored["content"] = serde_json::Value::String(history.content);
    let restored: AgentMessageDto = serde_json::from_value(restored).unwrap();
    assert_eq!(restored.into_domain(()).unwrap(), domain);
}

#[test]
fn legacy_assistant_replay_defaults_to_none_and_is_omitted() {
    for replay in [None, Some(serde_json::Value::Null)] {
        let mut wire = serde_json::json!({"type":"assistant","content":"legacy"});
        if let Some(replay) = replay {
            wire["replay"] = replay;
        }
        let dto: AgentMessageDto = serde_json::from_value(wire.clone()).unwrap();
        assert!(matches!(
            &dto,
            AgentMessageDto::Assistant { replay: None, .. }
        ));
        assert!(serde_json::to_value(&dto).unwrap().get("replay").is_none());
        let domain: AgentMessage = dto.into_domain(()).unwrap();
        assert!(matches!(
            &domain,
            AgentMessage::Assistant { replay: None, .. }
        ));
        let legacy_domain: AgentMessage = serde_json::from_value(wire).unwrap();
        assert_eq!(legacy_domain, domain);
        assert!(serde_json::to_value(domain)
            .unwrap()
            .get("replay")
            .is_none());
    }
}

#[test]
fn command_uses_arguments_and_source_only_from_conversion_context() {
    let message = ClientMessage::mcl_command(
        "command",
        &WorkspaceReferenceDto::new("demo", "/tmp/demo"),
        None,
        "INJECT ?",
        vec![serde_json::json!([{ "type": "user", "content": "hello" }])],
    );
    let value = serde_json::to_value(&message).unwrap();
    assert_eq!(value["type"], "mcl.command");
    assert!(value["message"].get("source").is_none());
    assert!(value["message"].get("binding").is_none());
    let ClientMessage::MclCommand {
        id: request_id,
        message,
    } = message
    else {
        unreachable!()
    };
    let command: MclCommand = message
        .into_domain((request_id, id("client", "real"), id("agent", "manager")))
        .unwrap();
    assert_eq!(command.source, id("client", "real"));
    assert_eq!(command.command.arguments.len(), 1);
}
#[test]
fn command_result_keeps_inference_stop_reason_usage_and_independent_history_error() {
    let event = MclCommandResult {
        id: "command".into(),
        source: id("client", "real"),
        target: id("agent", "manager"),
        result: Ok(MclValue::Inference(InferenceResponse {
            message: AgentMessage::Assistant {
                replay: None,
                reasoning: None,
                content: Some("partial".into()),
                tool_calls: vec![],
                stop_reason: None,
                usage: Some(TokenUsage {
                    input_tokens: 2,
                    output_tokens: 1,
                    cache_hit_tokens: 0,
                }),
            },
            stop_reason: StopReason::Length,
        })),
        history_error: Some(HistoryError::WriteFailed("database".into())),
    };
    let dto: MclCommandResultDto = (&event).into_dto(()).unwrap();
    let value = serde_json::to_value(ServerMessage::from(dto)).unwrap();
    assert_eq!(value["type"], "mcl.command_result");
    assert_eq!(value["result"]["Ok"]["Inference"]["stop_reason"], "length");
    assert_eq!(
        value["result"]["Ok"]["Inference"]["message"]["usage"]["input_tokens"],
        2
    );
    assert_eq!(value["history_error"]["kind"], "HistoryWriteFailed");
}
#[test]
fn typed_arrays_project_resources_without_private_snapshots() {
    use margatroid_types::{
        block_types::BlockInner,
        resource_components::{
            CallableDefinitionSnapshot, CallableResource, SandboxValue, ToolResource,
        },
    };
    let definitions = vec![CallableDefinitionSnapshot::Tool(ToolResource {
        alias: "original".into(),
        description: "tool".into(),
        input_schema: serde_json::json!({"type":"object"}),
        script_path: "/private/tool.lua".into(),
    })];
    let arrays = [
        BlockInner::Tool(
            definitions
                .into_iter()
                .enumerate()
                .map(|(index, definition)| CallableResource {
                    resource_id: id("tool", "resource"),
                    alias: format!("bound{index}"),
                    definition: std::sync::Arc::new(definition),
                })
                .collect(),
        ),
        BlockInner::Sandbox(vec![SandboxValue {
            resource_id: id("sandbox", "safe"),
            policy: "private-policy".into(),
        }]),
        BlockInner::Message(vec![AgentMessage::Inject {
            messages: vec![AgentMessage::System {
                content: "system".into(),
            }],
        }]),
    ];
    for array in arrays {
        let event = MclCommandResult {
            id: "get".into(),
            source: id("client", "real"),
            target: id("agent", "manager"),
            result: Ok(MclValue::Array(array)),
            history_error: None,
        };
        let dto: MclCommandResultDto = (&event).into_dto(()).unwrap();
        let value = dto.result.unwrap();
        assert_eq!(
            value,
            margatroid_types::mcl_projection::project_mcl_value(event.result.as_ref().unwrap())
                .unwrap()
        );
        let encoded = value.to_string();
        for private in [
            "private",
            "script_path",
            "content_path",
            "policy",
            "definition",
        ] {
            assert!(!encoded.contains(private), "{encoded}");
        }
        if let Some(tools) = value["Array"].get("Tool") {
            assert_eq!(tools[0]["alias"], "bound0");
            assert_eq!(tools[0]["description"], "tool");
            assert_eq!(tools[0].as_object().unwrap().len(), 4);
            assert!(serde_json::from_value::<CallableResource>(tools[0].clone()).is_err());
        }
        if let Some(sandboxes) = value["Array"].get("Sandbox") {
            assert_eq!(sandboxes[0].as_object().unwrap().len(), 1);
        }
        if let Some(messages) = value["Array"].get("Message") {
            assert_eq!(messages[0]["messages"][0]["type"], "system");
        }
    }
}
#[test]
fn import_projection_retains_kind_and_manual_array_arguments_are_not_decoded_as_snapshots() {
    use margatroid_types::block_types::InnerType;
    let event = MclCommandResult {
        id: "import".into(),
        source: id("client", "real"),
        target: id("agent", "manager"),
        result: Ok(MclValue::ResourceImport {
            kind: InnerType::Tool,
            resource_id: id("tool", "read"),
            alias: "reader".into(),
        }),
        history_error: None,
    };
    let dto: MclCommandResultDto = (&event).into_dto(()).unwrap();
    assert_eq!(dto.result.unwrap()["ResourceImport"]["kind"], "Tool");
    let dto = MclCommandDto {
        workspace: WorkspaceReferenceDto::new("demo", "/tmp/demo"),
        agent: None,
        command: "SET TOOLS ?".into(),
        arguments: vec![serde_json::json!(["reader"])],
    };
    let command: MclCommand = dto
        .into_domain((
            "manual".into(),
            id("client", "real"),
            id("agent", "manager"),
        ))
        .unwrap();
    assert_eq!(
        command.command.arguments,
        vec![serde_json::json!(["reader"])]
    );
}
#[test]
fn block_opened_serialization_keeps_exact_name_and_flags() {
    for (created, temporary) in [(true, false), (false, false), (true, true), (false, true)] {
        let event = MclCommandResult {
            id: "block".into(),
            source: id("client", "real"),
            target: id("agent", "manager"),
            result: Ok(MclValue::BlockOpened {
                block_id: "Case-sensitive.block".into(),
                created,
                temporary,
            }),
            history_error: None,
        };
        let expected = serde_json::json!({"BlockOpened": {"block_id": "Case-sensitive.block", "created": created, "temporary": temporary}});
        let dto: MclCommandResultDto = (&event).into_dto(()).unwrap();
        assert_eq!(dto.result.unwrap(), expected);
        assert_eq!(
            margatroid_types::mcl_projection::project_mcl_value(event.result.as_ref().unwrap())
                .unwrap(),
            expected
        );
        assert_eq!(
            serde_json::to_value(event.result.unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn registration_and_errors_use_correlated_bounded_shapes() {
    let registered = ServerMessage::ConnectionRegistered {
        id: "register".into(),
        client: ClientInfoDto {
            resource_id: ResourceIdDto("client:webui/console:1".into()),
            client_type: "webui".into(),
            name: "console".into(),
        },
    };
    let value = serde_json::to_value(registered).unwrap();
    assert_eq!(value["type"], "connection.registered");
    assert_eq!(value["client"]["resource_id"], "client:webui/console:1");
    let error = ProtocolErrorDto::new("InvalidRequest", "错误".repeat(1000));
    assert!(error.message.len() <= 512);
    let value = serde_json::to_value(ServerMessage::ConnectionRegisterFailed {
        id: "register".into(),
        error,
    })
    .unwrap();
    assert_eq!(value["error"]["kind"], "InvalidRequest");
}
#[test]
fn old_mailbox_and_unscoped_abort_entrypoints_are_not_accepted() {
    for kind in [
        "agent.assistant",
        "agent.turn.abort",
        "workflow.load",
        "agent.message",
    ] {
        assert!(serde_json::from_value::<ClientMessage>(
            serde_json::json!({"type": kind, "id": "old", "message": {}})
        )
        .is_err());
    }
    let domain: ResourceID = ResourceIdDto("tool:test/read".into())
        .into_domain(())
        .unwrap();
    assert_eq!(domain.to_string(), "tool:test/read:latest");
}
#[test]
fn message_conversion_enforces_depth_limit_without_a_type_whitelist() {
    let mut dto = AgentMessageDto::Error {
        message: "leaf".into(),
    };
    for _ in 0..34 {
        dto = AgentMessageDto::Inject {
            messages: vec![dto],
        };
    }
    assert!(IntoDomain::<AgentMessage>::into_domain(dto, ()).is_err());
}

#[test]
fn manager_resolution_rejects_stale_entity_generation_and_wrong_workspace() {
    use core_plugin::App;
    use margatroid_types::{
        agent_components::{Agent, AgentInfo, AgentState},
        block_storage::{
            AgentBlockStorage, AgentDatabaseHandle, BlockStorageError, BlockStorageRequest,
            StoredBlockResult,
        },
        execution_components::AgentExecutionScope,
        history_components::{AgentHistoryStore, HistoryError, HistoryRecord},
        lua_runtime_components::AgentLua,
        mcl_events::AgentMclQueue,
        resource_components::AgentDriverModelConfig,
        workspace_components::{
            Workspace, WorkspaceAgentDefinition, WorkspaceDefinition, WorkspaceError,
            WorkspaceErrorKind, WorkspaceState,
        },
        workspace_execution::spawn_workspace_execution,
    };
    use resource_id_plugin::WorldResourceExt;
    struct EmptyHistory;
    impl AgentHistoryStore for EmptyHistory {
        fn append_batch(&self, _: Vec<HistoryRecord>) -> Result<(), HistoryError> {
            Ok(())
        }
        fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
            Ok(vec![])
        }
    }
    impl AgentBlockStorage for EmptyHistory {
        fn execute(&self, _: BlockStorageRequest) -> Result<StoredBlockResult, BlockStorageError> {
            Err(BlockStorageError::InvalidRequest(
                "metadata-only fixture".into(),
            ))
        }
    }
    let mut app = App::new();
    app.add_plugin(resource_id_plugin::ResourcePlugin);
    let world = app.world_mut();
    world.register_library::<Workspace>("workspace").unwrap();
    world.register_library::<Agent>("agent").unwrap();
    let workspace = world.spawn();
    let (entry, close) = spawn_workspace_execution().unwrap();
    let definition = WorkspaceDefinition {
        id: id("workspace", "one"),
        name: "one".into(),
        manager: "manager".into(),
        project_root: "/tmp/one".into(),
        agents: vec![WorkspaceAgentDefinition {
            id: id("agent", "manager"),
            name: "manager".into(),
            driver: id("driver", "base"),
            home_root: "/tmp/home".into(),
            history_path: "/tmp/history".into(),
        }],
    };
    let reference: WorkspaceReferenceDto = (&definition).into_dto(()).unwrap();
    world
        .insert_library(
            workspace,
            Workspace::new(definition, WorkspaceState::Running, entry, close.clone()),
        )
        .unwrap();
    let make_agent = || {
        let database = AgentDatabaseHandle::new(EmptyHistory);
        Agent {
            resourceid: id("agent", "manager"),
            workspace: id("workspace", "one"),
            info: AgentInfo {
                model: AgentDriverModelConfig {
                    model: "model".into(),
                    temperature: None,
                    max_output_tokens: None,
                    top_p: None,
                    stop: vec![],
                },
                project_root: "/tmp/one".into(),
                home_root: "/tmp/home".into(),
                driver: id("driver", "base"),
                driver_dependencies: std::sync::Arc::from([]),
            },
            sandbox: Some(margatroid_types::resource_components::SandboxValue {
                resource_id: id("sandbox", "safe"),
                policy: "private-policy".into(),
            }),
            import_bindings: Default::default(),
            lua: AgentLua::default(),
            state: AgentState::Idle,
            history: database.history(),
            database,
            mcl: AgentMclQueue::default(),
            execution: AgentExecutionScope::new(),
            turn: None,
        }
    };
    let old = world.spawn();
    world.insert_library(old, make_agent()).unwrap();
    let model_runtime = margatroid_types::model_components::ModelRuntimeInfo {
        id: "model".into(),
        model: "upstream-model".into(),
        api_type: "chat_completions".into(),
        provider: None,
        context_window_tokens: 128_000,
        thinking: None,
        reasoning_effort: None,
    };
    world.insert_component(old, model_runtime.clone());
    world.insert_resource(
        margatroid_types::model_components::ModelRuntimeCatalog::new([
            margatroid_types::model_components::ModelRuntimeInfo {
                context_window_tokens: 256_000,
                ..model_runtime
            },
        ]),
    );
    world
        .get_component_mut::<Workspace>(workspace)
        .unwrap()
        .insert_agent(id("agent", "manager"), old);
    assert_eq!(resolve_agent(&world, &reference, None).unwrap().0, old);
    let old_state: AgentStateDto = old.into_dto(&*world).unwrap();
    assert_eq!(
        old_state.sandbox,
        Some(ResourceIdDto(id("sandbox", "safe").to_string()))
    );
    assert!(!serde_json::to_string(&old_state)
        .unwrap()
        .contains("private-policy"));
    assert!(old_state.error.is_none());
    assert_eq!(old_state.context_window_tokens, Some(128_000));
    assert_eq!(old_state.status, "idle");
    world.get_component_mut::<Agent>(old).unwrap().state = AgentState::Working;
    let working: AgentStateDto = old.into_dto(&*world).unwrap();
    assert_eq!(working.status, "working");
    assert!(working.turn.is_none());
    world.get_component_mut::<Agent>(old).unwrap().state = AgentState::Idle;
    let serialized = serde_json::to_value(&old_state).unwrap();
    assert_eq!(serialized["context_window_tokens"], 128_000);
    let mut legacy = serialized.clone();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("context_window_tokens");
    let compatible: AgentStateDto = serde_json::from_value(legacy).unwrap();
    assert!(compatible.context_window_tokens.is_none());
    assert!(serialized.get("error").is_none());
    let compatible: AgentStateDto = serde_json::from_value(serialized).unwrap();
    assert!(compatible.error.is_none());
    world.get_component_mut::<Agent>(old).unwrap().state = AgentState::Stopping {
        request_ids: vec![],
        vm_request_id: None,
    };
    world.insert_component(
        old,
        margatroid_types::agent_components::AgentFailure(
            margatroid_types::agent_events::AgentError::new(
                margatroid_types::agent_events::AgentErrorKind::DriverStopFailed,
                "driver failed: missing tool response",
            ),
        ),
    );
    let failed_state: AgentStateDto = old.into_dto(&*world).unwrap();
    assert_eq!(failed_state.status, "stopping");
    assert!(failed_state.turn.is_none());
    assert_eq!(
        failed_state.error.unwrap(),
        ProtocolErrorDto::new("DriverStopFailed", "driver failed: missing tool response",)
    );
    world.despawn(old);
    let new = world.spawn();
    world.insert_library(new, make_agent()).unwrap();
    assert!(resolve_agent(&world, &reference, None).is_err());
    world
        .get_component_mut::<Workspace>(workspace)
        .unwrap()
        .insert_agent(id("agent", "manager"), new);
    assert_eq!(resolve_agent(&world, &reference, None).unwrap().0, new);
    let new_state: AgentStateDto = new.into_dto(&*world).unwrap();
    // A fresh entity cannot inherit metadata from the old generation or catalog.
    assert!(new_state.context_window_tokens.is_none());
    assert!(serde_json::to_value(&new_state)
        .unwrap()
        .get("context_window_tokens")
        .is_none());
    assert_ne!(old_state.agent_instance, new_state.agent_instance);
    let mut wrong = reference.clone();
    wrong.project_root = "/tmp/wrong".into();
    assert!(resolve_agent(&world, &wrong, None).is_err());
    world
        .get_component_mut::<Workspace>(workspace)
        .unwrap()
        .set_state(WorkspaceState::Failed {
            error: WorkspaceError::new(WorkspaceErrorKind::InvalidRequest, "test cleanup"),
        });
    tokio_runtime_close(close);
}
fn tokio_runtime_close(close: margatroid_types::workspace_execution::WorkspaceExecutionClose) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(close.close())
        .unwrap();
}

#[test]
fn abort_wire_requires_turn_and_never_accepts_entity_identity() {
    let request = serde_json::json!({"type":"agent.turn.abort","id":"abort-1","message":{"workspace":{"id":"workspace:test/demo","name":"demo","project_root":"/tmp/demo"},"turn_id":"turn-1"}});
    let parsed: ClientMessage = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(parsed.id(), "abort-1");
    let ClientMessage::AgentTurnAbort { message, .. } = parsed else {
        panic!("wrong request")
    };
    assert!(message.agent.is_none());
    assert_eq!(message.turn_id, "turn-1");
    let mut missing = request;
    missing["message"]
        .as_object_mut()
        .unwrap()
        .remove("turn_id");
    assert!(serde_json::from_value::<ClientMessage>(missing).is_err());
    let response = ServerMessage::AgentTurnAbortResult {
        id: "abort-1".into(),
        workspace: message.workspace,
        agent: ResourceIdDto("agent:test/demo".into()),
        turn_id: message.turn_id,
        result: Ok(()),
    };
    let json = serde_json::to_value(response).unwrap();
    assert_eq!(json["type"], "agent.turn.abort_result");
    assert_eq!(json["result"], serde_json::json!({"Ok":null}));
}

#[test]
fn cancelled_assistant_round_trip_and_legacy_default() {
    let message = AgentMessage::aborted_assistant();
    let dto: AgentMessageDto = (&message).into_dto(()).unwrap();
    let wire = serde_json::to_value(&dto).unwrap();
    assert_eq!(wire["type"], "assistant");
    assert_eq!(wire["stop_reason"], "cancelled");
    assert!(wire["content"].is_null());
    assert!(wire["reasoning"].is_null());
    assert_eq!(dto.into_domain(()).unwrap(), message);
    let legacy: AgentMessageDto = serde_json::from_value(serde_json::json!({"type":"assistant","reasoning":null,"content":"legacy answer","tool_calls":[],"usage":null})).unwrap();
    assert!(matches!(
        legacy,
        AgentMessageDto::Assistant {
            stop_reason: None,
            ..
        }
    ));
}
