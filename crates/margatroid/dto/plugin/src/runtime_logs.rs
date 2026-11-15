use crate::{handler, types::*};
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use margatroid_protocol::*;
use serde_json::{Value, json};
use std::{any::Any, sync::Arc};
use storage_core::{
    AgentLogLevel, AgentLogRecord, AgentLogSource, RecordRange, RuntimeLogInput, RuntimeLogProbe,
    RuntimeLogStore, StoredRuntimeLog,
};

pub(crate) fn record(value: StoredRuntimeLog) -> RuntimeLogRecordDto {
    let row = value.input.record;
    RuntimeLogRecordDto {
        sequence: value.sequence,
        received_at_ms: value.received_at_ms,
        report_id: value.input.report_id,
        reporter: value.input.reporter,
        target: value.input.target,
        agent: value.input.agent,
        created_at_ms: row.created_at_ms.unwrap_or(value.received_at_ms),
        level: serde_json::to_value(row.level)
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
        source: serde_json::to_value(row.source)
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
        stage: row.stage,
        message: row.message,
        details_json: row.details_json,
        context_json: row.context_json,
        origin: row.origin,
        raw_error: row.raw_error,
        traceback: row.traceback,
    }
}
pub fn runtime_log_probe(
    clients: client_core::Clients,
    targets: &[config_core::OutboundTarget],
) -> RuntimeLogProbe {
    let targets = crate::sync_selectors(targets);
    RuntimeLogProbe::new(move |row| {
        match encode_server_message(&ServerMessage::LogSync {
            records: vec![record(row)],
        }) {
            Ok(frame) => {
                clients.send_ordered(&targets, Arc::new(client_core::ClientFrame::Text(frame)))
            }
            Err(error) => eprintln!("persisted runtime log could not be encoded for sync: {error}"),
        }
    })
}
fn storage_error(message: impl std::fmt::Display) -> storage_core::StorageError {
    storage_core::StorageError::new(storage_core::StorageErrorKind::InvalidRequest, message)
}
fn prepared_store(
    world: &World,
    connection: client_core::ConnectionId,
) -> Result<(RuntimeLogStore, Arc<client_core::ClientIdentity>), storage_core::StorageError> {
    handler::client(world, connection).map_err(|error| storage_error(error.message))?;
    let clients = world.get_resource::<client_core::Clients>().unwrap();
    let client = clients
        .registered(connection)
        .ok_or_else(|| storage_error("register connection first"))?;
    let store = world
        .get_resource::<RuntimeLogStore>()
        .cloned()
        .ok_or_else(|| storage_error("persistent runtime log store is not installed"))?;
    Ok((store, client))
}
pub(crate) fn handle(world: &mut World, route: &str, object: Box<dyn Any + Send>) {
    if route == "log.req" {
        let Ok(request) = object.downcast::<Prepared<
            RuntimeLogQueryDto,
            storage_core::RecordPage<StoredRuntimeLog>,
            storage_core::StorageError,
        >>() else {
            return;
        };
        let store = match prepared_store(world, request.ticket.connection) {
            Ok((store, _)) => store,
            Err(error) => {
                handler::deliver(&request.reply, Err(error));
                return;
            }
        };
        let range = RecordRange {
            after_sequence: request.input.after_sequence,
            before_sequence: request.input.before_sequence,
            limit: request.input.limit.unwrap_or(100),
        };
        world.send_async_closure(
            app_runtime_plugin::RuntimePlugin::UPDATE,
            move || async move {
                let result = tokio::task::spawn_blocking(move || store.read(range))
                    .await
                    .unwrap_or_else(|error| Err(storage_error(error)));
                handler::deliver(&request.reply, result);
                Ok::<(), DtoAsyncError>(())
            },
        );
    } else {
        let Ok(request) = object.downcast::<Prepared<
            ClientLogReportDto,
            StoredRuntimeLog,
            storage_core::StorageError,
        >>() else {
            return;
        };
        let (store, client) = match prepared_store(world, request.ticket.connection) {
            Ok(value) => value,
            Err(error) => {
                handler::deliver(&request.reply, Err(error));
                return;
            }
        };
        let reply = request.reply.clone();
        let input = request.input;
        let mut context = input.context_json.unwrap_or_else(|| json!({}));
        context["reporting_client"] =
            json!({"address":client.address,"client_type":client.client_type,"name":client.name});
        let level = match input.level.as_str() {
            "debug" => AgentLogLevel::Debug,
            "info" => AgentLogLevel::Info,
            "warn" => AgentLogLevel::Warn,
            _ => AgentLogLevel::Error,
        };
        let report = RuntimeLogInput {
            report_id: Some(input.report_id),
            reporter: client.address.clone(),
            target: input.target,
            agent: input.agent,
            record: AgentLogRecord {
                created_at_ms: Some(input.created_at_ms),
                level,
                source: AgentLogSource::Host,
                stage: "client".into(),
                message: input.message,
                details_json: input.details_json,
                context_json: Some(context),
                origin: input.origin,
                raw_error: input.raw_error,
                traceback: input.traceback,
            },
        };
        world.send_async_closure(
            app_runtime_plugin::RuntimePlugin::UPDATE,
            move || async move {
                let result = tokio::task::spawn_blocking(move || store.append(report))
                    .await
                    .unwrap_or_else(|error| Err(storage_error(error)));
                handler::deliver(&reply, result);
                Ok::<(), DtoAsyncError>(())
            },
        );
    }
}
pub(crate) fn page(
    page: storage_core::RecordPage<StoredRuntimeLog>,
) -> Result<Value, ProtocolErrorDto> {
    Ok(
        json!({"records":page.records.into_iter().map(record).collect::<Vec<_>>(),"next_before_sequence":page.next_before_sequence,"next_after_sequence":page.next_after_sequence}),
    )
}

pub(crate) async fn report_response(
    state: &DtoState,
    connection: client_core::ConnectionId,
    request: Value,
    response: &Value,
    diagnostic: Option<String>,
) {
    if response["success"] != false {
        return;
    }
    let store = state.logs.clone();
    let client = state.clients.registered(connection);
    let failure = response["error"].clone();
    let raw = diagnostic.unwrap_or_else(|| serde_json::to_string(&failure).unwrap_or_default());
    let input = RuntimeLogInput {
        report_id: None,
        reporter: "backend".into(),
        target: request["type"].as_str().unwrap_or("protocol").into(),
        agent: request["message"]["address"].as_str().map(str::to_owned),
        record: AgentLogRecord {
            created_at_ms: None,
            level: if failure["code"] == "AgentUiNotReady" {
                AgentLogLevel::Warn
            } else {
                AgentLogLevel::Error
            },
            source: AgentLogSource::Host,
            stage: "response".into(),
            message: failure["message"]
                .as_str()
                .unwrap_or("request failed")
                .into(),
            details_json: Some(json!({"request":request,"response":response})),
            context_json: Some(
                json!({"connection":connection.get(),"client":client.map(|client|client.address.clone()),"request_id":response["id"]}),
            ),
            origin: Some("dto_plugin::serve_connection".into()),
            raw_error: Some(raw),
            traceback: Some(std::backtrace::Backtrace::force_capture().to_string()),
        },
    };
    if let Err(error) = tokio::task::spawn_blocking(move || store.append(input))
        .await
        .map_err(|error| error.to_string())
        .and_then(|value| value.map_err(|error| error.to_string()))
    {
        eprintln!("backend response diagnostic persistence failed: {error}");
    }
}
pub(crate) fn report_ui(
    store: &RuntimeLogStore,
    address: &str,
    origin: &std::path::Path,
    state: &agent_driver_core::DriverUiState,
) {
    let (level, message, raw) = match state {
        agent_driver_core::DriverUiState::Initializing => (
            AgentLogLevel::Warn,
            "Agent UI initialization is in progress".to_owned(),
            None,
        ),
        agent_driver_core::DriverUiState::Ready(bundle) => (
            AgentLogLevel::Info,
            if bundle.is_some() {
                "Agent UI resources are ready"
            } else {
                "Agent has no UI resource bundle"
            }
            .to_owned(),
            None,
        ),
        agent_driver_core::DriverUiState::Failed(error) => (
            AgentLogLevel::Error,
            error.to_string(),
            Some(format!("{error:?}")),
        ),
    };
    let details = match state {
        agent_driver_core::DriverUiState::Initializing => json!({"state":"initializing"}),
        agent_driver_core::DriverUiState::Ready(bundle) => {
            json!({"state":"ready","revision":bundle.as_ref().map(|value|&value.revision),"entry":bundle.as_ref().map(|value|&value.entry),"files":bundle.as_ref().map(|value|value.files.iter().map(|file|json!({"path":file.path,"media_type":file.media_type,"bytes":file.data.len()})).collect::<Vec<_>>())})
        }
        agent_driver_core::DriverUiState::Failed(error) => {
            json!({"state":"failed","cause":format!("{error:?}")})
        }
    };
    let input = RuntimeLogInput {
        report_id: None,
        reporter: "backend".into(),
        target: "agent.ui".into(),
        agent: Some(address.into()),
        record: AgentLogRecord {
            created_at_ms: None,
            level,
            source: AgentLogSource::Host,
            stage: "ui".into(),
            message,
            details_json: Some(details),
            context_json: Some(json!({"agent":address})),
            origin: Some(origin.to_string_lossy().into_owned()),
            raw_error: raw,
            traceback: if level == AgentLogLevel::Error {
                Some(std::backtrace::Backtrace::force_capture().to_string())
            } else {
                None
            },
        },
    };
    if let Err(error) = store.append(input) {
        eprintln!("UI diagnostic persistence failed: {error}");
    }
}
