use super::*;

pub(super) fn should_diagnose(failure: &ToolError) -> bool {
    !matches!(
        failure.kind,
        Kind::Cancelled | Kind::Timeout | Kind::LogFailed
    )
}

fn typed_failure(failure: &mlua::Error) -> Option<ToolError> {
    match failure {
        mlua::Error::ExternalError(source) => {
            if let Some(failure) = source.downcast_ref::<ToolError>() {
                return Some(failure.clone());
            }
            if let Some(failure) = source.downcast_ref::<mlua::Error>() {
                return typed_failure(failure);
            }
            source
                .downcast_ref::<host::SharedHostError>()
                .map(|failure| {
                    use host::SharedHostErrorKind as S;
                    let kind = match failure.kind {
                        S::InvalidArguments => Kind::InvalidArguments,
                        S::InvalidRequest | S::InvalidExecutionContext | S::Unavailable => {
                            Kind::InvalidRequest
                        }
                        S::RunnerStartFailed => Kind::RunnerStartFailed,
                        S::CleanupFailed => Kind::CleanupFailed,
                        S::Closed => Kind::Closed,
                        S::ExecutionFailed => Kind::ExecutionFailed,
                    };
                    error(kind, failure.description.clone())
                })
        }
        mlua::Error::CallbackError { cause, .. }
        | mlua::Error::WithContext { cause, .. }
        | mlua::Error::BadArgument { cause, .. } => typed_failure(cause),
        _ => None,
    }
}

pub(super) fn lua_failure(failure: mlua::Error) -> ToolError {
    if let Some(typed) = typed_failure(&failure) {
        return typed;
    }
    match &failure {
        mlua::Error::CallbackError { cause, .. } | mlua::Error::WithContext { cause, .. } => {
            lua_failure((**cause).clone())
        }
        mlua::Error::BadArgument { .. } => {
            error(Kind::InvalidArguments, "invalid Lua method arguments")
        }
        mlua::Error::SyntaxError { .. } => error(
            Kind::InvalidDefinition,
            host::LuaFailureDiagnostic::capture(&failure).message,
        ),
        mlua::Error::RuntimeError(_) => error(
            Kind::ExecutionFailed,
            host::LuaFailureDiagnostic::capture(&failure).message,
        ),
        _ => error(
            Kind::ExecutionFailed,
            "Lua execution failed; see agent logs",
        ),
    }
}

pub(super) fn diagnostic_record(
    context: &ToolContext,
    stage: &str,
    failure: &mlua::Error,
    brief: &ToolError,
) -> AgentLogRecord {
    let raw = failure.to_string();
    let captured = host::LuaFailureDiagnostic::capture_with(failure, |source| {
        source
            .downcast_ref::<ToolError>()
            .map(|failure| failure.message.clone())
    });
    let traceback = captured.traceback;
    AgentLogRecord {
        created_at_ms: None,
        level: AgentLogLevel::Error,
        source: AgentLogSource::Tool,
        stage: stage.to_owned(),
        message: brief.message.clone(),
        details_json: Some(serde_json::json!({"class": format!("{:?}", captured.class)})),
        context_json: Some(
            serde_json::json!({"agent": context.agent_name, "turn": context.turn,
            "task_id": context.task_id, "call_id": context.call_id, "alias": context.alias,
            "resource_path": context.source_origin, "stage": stage}),
        ),
        origin: Some(context.source_origin.clone()),
        raw_error: Some(raw),
        traceback,
    }
}

pub(super) async fn persist_log(
    context: &ToolExecutionContext,
    mut record: AgentLogRecord,
) -> Result<(), ToolError> {
    let mut metadata = record
        .context_json
        .take()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    metadata.extend(serde_json::json!({"agent": context.agent_name, "turn": context.request.turn,
        "task_id": context.request.id, "call_id": context.request.call_id, "alias": context.request.alias,
        "resource_path": context.tool_entry.path, "stage": record.stage}).as_object().unwrap().clone());
    record.context_json = Some(Value::Object(metadata));
    let storage = context.storage.clone();
    tokio::task::spawn_blocking(move || storage_core::append_agent_log(&storage, record))
        .await
        .map_err(|failure| {
            eprintln!("tool log writer join failed: {failure}");
            error(Kind::LogFailed, "agent log persistence failed")
        })?
        .map_err(|failure| {
            eprintln!("tool log persistence failed: {failure}");
            error(Kind::LogFailed, "agent log persistence failed")
        })?;
    Ok(())
}

pub(super) async fn report_startup_failure<R: AsyncBufRead + Unpin>(
    request: &RunnerRequest,
    input: &mut R,
    stage: &str,
    failure: mlua::Error,
    kind: Kind,
) -> ToolError {
    let mut brief = lua_failure(failure.clone());
    if brief.kind == Kind::ExecutionFailed && typed_failure(&failure).is_none() {
        brief.kind = kind;
    }
    if should_diagnose(&brief) {
        let record = diagnostic_record(&request.context, stage, &failure, &brief);
        let request_id = "startup-diagnostic".to_owned();
        let sent = write_json(
            &mut tokio::io::stdout(),
            &RunnerWire {
                protocol_version: 1,
                frame: RunnerFrame::LuaDiagnostic {
                    request_id: request_id.clone(),
                    record,
                },
            },
        )
        .await;
        let acknowledged = if sent.is_ok() {
            matches!(read_json::<HostWire, _>(input).await, Ok(Some(HostWire { protocol_version: 1,
                frame: HostFrame::Response { request_id: id, result: Ok(_) } })) if id == request_id)
        } else {
            false
        };
        if !acknowledged {
            eprintln!("tool startup diagnostic transport failed; original: {failure}");
        }
    }
    brief
}

pub(super) fn install_log_callback(
    lua: &Lua,
    callbacks: &mlua::Table,
    bridge: SharedBridge,
    context: ToolContext,
) -> mlua::Result<()> {
    callbacks.set(
        "log",
        lua.create_async_function(
            move |lua, (level, message, details): (String, String, Option<LuaValue>)| {
                let bridge = bridge.clone();
                let context = context.clone();
                async move {
                    let invalid =
                        |message| mlua::Error::external(error(Kind::InvalidArguments, message));
                    let level: AgentLogLevel = serde_json::from_value(Value::String(level))
                        .map_err(|_| invalid("log level must be debug, info, warn or error"))?;
                    let details_json = match details {
                        None | Some(LuaValue::Nil) => None,
                        Some(value) => Some(data::from_lua_object(&lua, value)?),
                    };
                    let record = AgentLogRecord {
                        created_at_ms: None,
                        level,
                        source: AgentLogSource::Tool,
                        stage: "execute".to_owned(),
                        message,
                        details_json,
                        context_json: None,
                        origin: Some(context.source_origin),
                        raw_error: None,
                        traceback: None,
                    };
                    bridge
                        .call(|request_id| RunnerFrame::Log { request_id, record })
                        .await
                        .map_err(|failure| {
                            eprintln!("tool log transport or persistence failed: {failure}");
                            mlua::Error::external(error(
                                Kind::LogFailed,
                                "agent log persistence failed",
                            ))
                        })?;
                    Ok(())
                }
            },
        )?,
    )
}

pub(super) async fn report_host_failure(context: &mut ToolExecutionContext, failure: &ToolError) {
    if context.diagnostic_received
        || !matches!(
            failure.kind,
            Kind::RunnerStartFailed
                | Kind::RunnerProtocolFailed
                | Kind::SandboxUnavailable
                | Kind::ExecutionFailed
                | Kind::ReadFailed
                | Kind::TaskPanicked
        )
    {
        return;
    }
    context.diagnostic_received = true;
    let record = AgentLogRecord {
        created_at_ms: None,
        level: AgentLogLevel::Error,
        source: AgentLogSource::Host,
        stage: if context.configuration.is_some() {
            "execute"
        } else {
            "startup"
        }
        .to_owned(),
        message: failure.message.clone(),
        details_json: None,
        context_json: None,
        origin: Some(context.tool_entry.path.to_string_lossy().into_owned()),
        raw_error: Some(failure.to_string()),
        traceback: None,
    };
    if let Err(log_failure) = persist_log(context, record).await {
        eprintln!("tool host diagnostic persistence failed: {log_failure}; original: {failure}");
    }
}
