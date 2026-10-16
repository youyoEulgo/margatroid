use crate::{
    types::{
        BackendStateReportCache, PendingAbortResponse, PendingMclCommandResponse,
        PendingMclCommandResponses, PendingSnapshot, PendingWorkspaceResponse,
    },
    WebSocketMessageSend,
};
use app_runtime_plugin::{RuntimeEventSender, WorldEventExt};
use async_runtime_plugin::WorldAsyncExt;
use config_plugin::{MargatroidConfig, WebSocketMessageTarget};
use core_plugin::World;
use log_plugin::{TracingStream, TracingStreamError};
use margatroid_protocol::AgentStateDto;
use margatroid_protocol::{
    resolve_agent, resolve_workspace, AgentHistoryDto, BackendStateDto, ClientInfoDto,
    ClientMessage, IntoDomain, IntoDto, LogRecordDto, MclCommandResultDto, ProtocolError,
    ProtocolErrorDto, ProtocolErrorKind, ServerMessage, WorkspaceInfoDto,
};
use margatroid_types::{
    agent_components::{Agent, AgentState},
    agent_events::{AbortAgentTurn, AbortAgentTurnResult},
    dto_events::DtoHistorySnapshotCompleted,
    history_components::AgentHistoryHandle,
};
use margatroid_types::{
    client_components::{Client, WorldClientExt},
    client_events::ClientRegistrationResult,
    mcl_events::MclCommandResult,
    workspace_components::WorldWorkspaceExt,
    workspace_events::{StartWorkspaceResult, StopWorkspaceResult},
};
use server_plugin::{
    ServerFailed, ServerStarted, ServerStopped, WebSocketConnected, WebSocketConnectionId,
    WebSocketConnections, WebSocketDisconnected, WebSocketMessage, WebSocketMessageSender,
    WebSocketProtocolFailed,
};

pub(crate) fn handle_inbound_message(
    world: &mut World,
    connection_id: WebSocketConnectionId,
    message: WebSocketMessage,
) {
    let WebSocketMessage::Text(text) = message else {
        return;
    };
    if text.len() > 1024 * 1024 {
        return;
    }
    let request = match serde_json::from_str::<ClientMessage>(&text) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(connection = connection_id.get(), error = %error, "invalid API request");
            return;
        }
    };
    let id = request.id().to_owned();
    let result = route_request(world, connection_id, request);
    if let Err(error) = result {
        send_to_connection(
            world,
            connection_id,
            &ServerMessage::RequestFailed {
                id,
                error: error.into(),
            },
        );
    }
}
fn operation_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "dto:workspace:{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
fn route_request(
    world: &mut World,
    connection_id: WebSocketConnectionId,
    request: ClientMessage,
) -> Result<(), ProtocolError> {
    if request.id().trim().is_empty() || request.id().len() > 512 {
        return Err(ProtocolError::new(
            ProtocolErrorKind::InvalidRequest,
            "invalid request ID",
        ));
    }
    if let ClientMessage::ConnectionRegister { id, message } = request {
        world.send_event(message.into_domain((id, connection_id))?);
        return Ok(());
    }
    let (client, identity) = world
        .client_by_connection(connection_id)
        .map(|(entity, client)| (entity, client.resource_id.clone()))
        .ok_or_else(|| {
            ProtocolError::new(ProtocolErrorKind::Unregistered, "connection not registered")
        })?;
    let pending = world
        .get_resource::<PendingMclCommandResponses>()
        .expect("DTO pending responses missing");
    if pending
        .aborts
        .iter()
        .any(|p| p.client == client && p.id == request.id())
        || pending
            .commands
            .iter()
            .any(|p| p.source == identity && p.id == request.id())
        || pending
            .workspaces
            .iter()
            .any(|p| p.client == client && p.id == request.id())
        || pending
            .snapshots
            .iter()
            .any(|p| p.client == client && p.id == request.id())
    {
        return Err(ProtocolError::new(
            ProtocolErrorKind::DuplicateRequest,
            "request already in flight",
        ));
    }
    match request {
        ClientMessage::ConnectionRegister { .. } => unreachable!(),
        ClientMessage::WorkspaceStart { id, message } => {
            let operation_id = operation_id();
            let event = message.into_domain(operation_id.clone())?;
            let reference = (&event.definition).into_dto(())?;
            world
                .get_resource_mut::<PendingMclCommandResponses>()
                .unwrap()
                .workspaces
                .push(PendingWorkspaceResponse {
                    id,
                    operation_id,
                    connection_id,
                    client,
                    stopping: false,
                    reference,
                });
            world.send_event(event);
        }
        ClientMessage::WorkspaceStop { id, message } => {
            let entity = resolve_workspace(world, &message.workspace)?;
            let operation_id = operation_id();
            world
                .workspace_stop(operation_id.clone(), entity)
                .map_err(|error| {
                    ProtocolError::new(ProtocolErrorKind::InvalidRequest, error.to_string())
                })?;
            world
                .get_resource_mut::<PendingMclCommandResponses>()
                .unwrap()
                .workspaces
                .push(PendingWorkspaceResponse {
                    id,
                    operation_id,
                    connection_id,
                    client,
                    stopping: true,
                    reference: message.workspace,
                });
        }
        ClientMessage::AgentTurnAbort { id, message } => {
            if message.turn_id.trim().is_empty() || message.turn_id.len() > 512 {
                return Err(ProtocolError::new(
                    ProtocolErrorKind::InvalidRequest,
                    "invalid turn ID",
                ));
            }
            let (entity, resource_id) =
                resolve_agent(world, &message.workspace, message.agent.as_ref())?;
            let agent = world.get_component::<Agent>(entity).unwrap();
            let instance = agent.lua.instance.clone().ok_or_else(|| {
                ProtocolError::new(ProtocolErrorKind::InvalidRequest, "agent VM is not ready")
            })?;
            let workspace = agent.workspace.clone();
            let operation_id = operation_id();
            let event = AbortAgentTurn {
                id: operation_id.clone(),
                agent: entity,
                resource_id: resource_id.clone(),
                workspace,
                instance: instance.clone(),
                expected_turn_id: message.turn_id.clone(),
            };
            world
                .get_resource_mut::<PendingMclCommandResponses>()
                .unwrap()
                .aborts
                .push(PendingAbortResponse {
                    id,
                    operation_id,
                    connection_id,
                    client,
                    agent: entity,
                    resource_id,
                    workspace: message.workspace,
                    instance,
                    turn_id: message.turn_id,
                });
            world.send_event(event);
        }
        ClientMessage::MclCommand { id, message } => {
            let (entity, target) =
                resolve_agent(world, &message.workspace, message.agent.as_ref())?;
            if matches!(
                world.get_component::<Agent>(entity).unwrap().state,
                AgentState::Working
            ) && is_user_injection(&message.command, &message.arguments)
            {
                return Err(ProtocolError::new(
                    ProtocolErrorKind::InvalidRequest,
                    "cannot inject a user message while the agent is working",
                ));
            }
            let event = message.into_domain((id.clone(), identity.clone(), target.clone()))?;
            world
                .get_resource_mut::<PendingMclCommandResponses>()
                .unwrap()
                .commands
                .push(PendingMclCommandResponse {
                    id,
                    connection_id,
                    client,
                    source: identity,
                    target,
                });
            world.send_event(event);
        }
        ClientMessage::StateQuery { id, .. } => {
            let state: BackendStateDto = ().into_dto(&*world)?;
            let histories = prepare_histories(world, None)?;
            queue_snapshot(world, connection_id, client, id, false, state, histories);
        }
        ClientMessage::HistoryQuery { id, message } => {
            let (entity, _) = resolve_agent(world, &message.workspace, message.agent.as_ref())?;
            let histories = prepare_histories(world, Some(entity))?;
            let state = BackendStateDto {
                agents: histories
                    .iter()
                    .map(|(metadata, _)| metadata.clone())
                    .collect(),
                ..BackendStateDto::default()
            };
            queue_snapshot(world, connection_id, client, id, true, state, histories);
        }
    }
    Ok(())
}
fn is_user_injection(command: &str, arguments: &[serde_json::Value]) -> bool {
    fn contains_user(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Array(values) => values.iter().any(contains_user),
            serde_json::Value::Object(value) => {
                value.get("type").and_then(|value| value.as_str()) == Some("user")
                    || value.get("messages").is_some_and(contains_user)
            }
            _ => false,
        }
    }
    let words: Vec<_> = command.split_whitespace().collect();
    words.len() == 4
        && words[0].eq_ignore_ascii_case("INJECT")
        && words[1] == "?"
        && words[2].eq_ignore_ascii_case("TO")
        && arguments.iter().any(contains_user)
}
fn response_is_live<C: Copy + Eq + std::hash::Hash>(
    live: &std::collections::HashSet<(C, core_plugin::Entity)>,
    connection: C,
    client: core_plugin::Entity,
) -> bool {
    live.contains(&(connection, client))
}
fn purge_disconnected_responses(world: &mut World) {
    let disconnected: std::collections::HashSet<_> = world
        .event_reader::<WebSocketDisconnected>()
        .into_iter()
        .map(|event| event.connection_id)
        .collect();
    let live: std::collections::HashSet<_> = world
        .query_with::<Client>()
        .result()
        .into_iter()
        .filter_map(|entity| {
            let client = world.get_component::<Client>(entity)?;
            let connected = world
                .get_resource::<WebSocketConnections>()
                .and_then(|connections| connections.get(client.connection_id))
                .is_some();
            (connected && !disconnected.contains(&client.connection_id))
                .then_some((client.connection_id, entity))
        })
        .collect();
    let pending = world
        .get_resource_mut::<PendingMclCommandResponses>()
        .unwrap();
    pending
        .aborts
        .retain(|pending| response_is_live(&live, pending.connection_id, pending.client));
    pending
        .commands
        .retain(|pending| response_is_live(&live, pending.connection_id, pending.client));
    pending
        .workspaces
        .retain(|pending| response_is_live(&live, pending.connection_id, pending.client));
    pending
        .snapshots
        .retain(|pending| response_is_live(&live, pending.connection_id, pending.client));
}
pub(crate) fn handle_pending_mcl_responses(world: &mut World) {
    purge_disconnected_responses(world);
    let aborts: Vec<_> = world
        .event_reader::<AbortAgentTurnResult>()
        .into_iter()
        .cloned()
        .collect();
    for result in aborts {
        let pending = world
            .get_resource_mut::<PendingMclCommandResponses>()
            .unwrap();
        let Some(index) = pending.aborts.iter().position(|p| {
            p.operation_id == result.id
                && p.agent == result.agent
                && p.resource_id == result.resource_id
                && p.workspace.id.0 == result.workspace.to_string()
                && p.instance == result.instance
                && p.turn_id == result.expected_turn_id
        }) else {
            continue;
        };
        let pending = pending.aborts.remove(index);
        if world
            .client_by_connection(pending.connection_id)
            .is_some_and(|(entity, _)| entity == pending.client)
        {
            send_to_connection(
                world,
                pending.connection_id,
                &ServerMessage::AgentTurnAbortResult {
                    id: pending.id,
                    workspace: pending.workspace,
                    agent: pending.resource_id.into_dto(()).unwrap(),
                    turn_id: pending.turn_id,
                    result: result.result.map_err(|error| {
                        ProtocolErrorDto::new(format!("{:?}", error.kind), error.message)
                    }),
                },
            );
        }
    }
    let results: Vec<_> = world
        .event_reader::<MclCommandResult>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        let pending = world
            .get_resource_mut::<PendingMclCommandResponses>()
            .unwrap();
        let Some(index) = pending.commands.iter().position(|p| {
            p.id == result.id && p.source == result.source && p.target == result.target
        }) else {
            continue;
        };
        let pending = pending.commands.remove(index);
        let valid = world
            .client_by_connection(pending.connection_id)
            .is_some_and(|(entity, client)| {
                entity == pending.client && client.resource_id == pending.source
            });
        if valid {
            if let Ok(dto) = MclCommandResultDto::from_domain(&result, ()) {
                send_to_connection(world, pending.connection_id, &dto.into());
            }
        }
    }
}
use margatroid_protocol::FromDomain;
pub(crate) fn handle_collect_external_events(world: &mut World) {
    let targets = world
        .get_resource::<MargatroidConfig>()
        .cloned()
        .expect("DTO config missing");
    report_server_events(world);
    report_client_registrations(world);
    report_workspace_results(world);
    if !world.event_reader::<MclCommandResult>().is_empty() {
        world
            .get_resource_mut::<BackendStateReportCache>()
            .unwrap()
            .dirty = true;
    }
    report_backend_state(world, targets.backend_state());
}
fn report_client_registrations(world: &World) {
    for event in world.event_reader::<ClientRegistrationResult>() {
        let message = match &event.result {
            Ok(registered) => {
                let Some(client) = world.get_component::<Client>(registered.client) else {
                    continue;
                };
                if client.connection_id != event.connection_id
                    || client.resource_id != registered.resource_id
                {
                    continue;
                }
                ServerMessage::ConnectionRegistered {
                    id: event.id.clone(),
                    client: ClientInfoDto {
                        resource_id: registered.resource_id.clone().into_dto(()).unwrap(),
                        client_type: client.client_type.clone(),
                        name: client.name.clone(),
                    },
                }
            }
            Err(error) => ServerMessage::ConnectionRegisterFailed {
                id: event.id.clone(),
                error: ProtocolErrorDto::new("ClientRegistrationFailed", error.to_string()),
            },
        };
        send_to_connection(world, event.connection_id, &message);
    }
}
fn report_workspace_results(world: &mut World) {
    let starts: Vec<_> = world
        .event_reader::<StartWorkspaceResult>()
        .into_iter()
        .cloned()
        .collect();
    let stops: Vec<_> = world
        .event_reader::<StopWorkspaceResult>()
        .into_iter()
        .cloned()
        .collect();
    for (id, stopping, result) in starts
        .into_iter()
        .map(|e| (e.id, false, e.result))
        .chain(stops.into_iter().map(|e| (e.id, true, e.result)))
    {
        let pending = world
            .get_resource_mut::<PendingMclCommandResponses>()
            .unwrap();
        let Some(index) = pending
            .workspaces
            .iter()
            .position(|p| p.operation_id == id && p.stopping == stopping)
        else {
            continue;
        };
        let pending = pending.workspaces.remove(index);
        if !world
            .client_by_connection(pending.connection_id)
            .is_some_and(|(entity, _)| entity == pending.client)
        {
            continue;
        }
        let id = pending.id;
        let message = match (stopping, result) {
            (false, Ok(entity)) => {
                match IntoDto::<WorkspaceInfoDto, _>::into_dto(entity, &*world) {
                    Ok(workspace) => ServerMessage::WorkspaceStarted { id, workspace },
                    Err(error) => ServerMessage::WorkspaceStartFailed {
                        id,
                        error: error.into(),
                    },
                }
            }
            (false, Err(error)) => ServerMessage::WorkspaceStartFailed {
                id,
                error: ProtocolErrorDto::new("WorkspaceStartFailed", error.to_string()),
            },
            (true, Ok(_)) => ServerMessage::WorkspaceStopped {
                id,
                workspace: pending.reference,
            },
            (true, Err(error)) => ServerMessage::WorkspaceStopFailed {
                id,
                error: ProtocolErrorDto::new("WorkspaceStopFailed", error.to_string()),
            },
        };
        send_to_connection(world, pending.connection_id, &message);
    }
}
fn send_to_connection(
    world: &World,
    connection_id: WebSocketConnectionId,
    message: &ServerMessage,
) {
    let Some(sender) = world
        .get_resource::<WebSocketConnections>()
        .and_then(|connections| connections.get(connection_id))
    else {
        return;
    };
    if let Ok(encoded) = serde_json::to_string(message) {
        let _ = sender.try_send(WebSocketMessage::Text(encoded.into()));
    }
}
pub(crate) fn handle_outbound_messages(world: &mut World, outgoing: Vec<WebSocketMessageSend>) {
    let Some(connections) = world.get_resource::<WebSocketConnections>().cloned() else {
        return;
    };
    for outgoing in outgoing {
        let is_log = outgoing
            .message
            .get("type")
            .and_then(serde_json::Value::as_str)
            == Some("log");
        let encoded = match serde_json::to_string(&outgoing.message) {
            Ok(encoded) => encoded,
            Err(error) => {
                tracing::warn!(error = %error, "API response serialization failed");
                continue;
            }
        };
        let senders = match &outgoing.target {
            WebSocketMessageTarget::Broadcast => connections.get_all(),
            WebSocketMessageTarget::Type(connection_type) => {
                connections.get_by_type(connection_type)
            }
            WebSocketMessageTarget::Name(name) => match connections.get_by_name(name) {
                Some(sender) => vec![sender],
                None => {
                    tracing::warn!(name = %name, "named WebSocket connection was not found");
                    Vec::new()
                }
            },
        };
        for (connection_id, result) in
            WebSocketMessageSender::new(senders, WebSocketMessage::Text(encoded.into())).try_send()
        {
            if let Err(error) = result {
                if !is_log {
                    tracing::warn!(connection = connection_id.get(), error = %error, "WebSocket API message could not be queued");
                }
            }
        }
    }
}

fn report_server_events(world: &World) {
    for event in world.event_reader::<ServerStarted>() {
        tracing::info!(address = %event.address, "daemon WebSocket server started");
    }
    for event in world.event_reader::<ServerFailed>() {
        tracing::error!(error = %event.message, "daemon WebSocket server failed");
    }
    if !world.event_reader::<ServerStopped>().is_empty() {
        tracing::info!("daemon WebSocket server stopped");
    }
    for event in world.event_reader::<WebSocketConnected>() {
        tracing::info!(
            connection = event.connection_id.get(),
            "WebSocket connection opened"
        );
    }
    for event in world.event_reader::<WebSocketDisconnected>() {
        if let Some(reason) = &event.reason {
            tracing::info!(
                connection = event.connection_id.get(),
                code = reason.code(),
                reason = reason.reason(),
                "WebSocket connection closed"
            );
        } else {
            tracing::info!(
                connection = event.connection_id.get(),
                "WebSocket connection closed"
            );
        }
    }
    for event in world.event_reader::<WebSocketProtocolFailed>() {
        tracing::warn!(connection = event.connection_id.get(), error = %event.error, "WebSocket protocol failed");
    }
}

type HistoryRead = (AgentStateDto, AgentHistoryHandle);

fn prepare_histories(
    world: &World,
    selected: Option<core_plugin::Entity>,
) -> Result<Vec<HistoryRead>, ProtocolError> {
    let mut entities = selected
        .map(|entity| vec![entity])
        .unwrap_or_else(|| world.query_with::<Agent>().result());
    entities.sort_by_key(|entity| {
        world
            .get_component::<Agent>(*entity)
            .unwrap()
            .resourceid
            .to_string()
    });
    entities
        .into_iter()
        .map(|entity| {
            Ok((
                entity.into_dto(world)?,
                world
                    .get_component::<Agent>(entity)
                    .unwrap()
                    .history
                    .clone(),
            ))
        })
        .collect()
}

fn spawn_snapshot(
    world: &World,
    id: String,
    mut state: BackendStateDto,
    histories: Vec<HistoryRead>,
) {
    let events = world.event_sender();
    world.spawn_async_service(async move {
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            state.histories = histories
                .into_iter()
                .map(|(metadata, handle)| {
                    let records = handle.list().map_err(|error| error.to_string())?;
                    AgentHistoryDto::from_domain((metadata, records), ())
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<_, _>>()?;
            serde_json::to_value(state).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result);
        events.send_event(DtoHistorySnapshotCompleted { id, result });
    });
}

fn queue_snapshot(
    world: &mut World,
    connection_id: WebSocketConnectionId,
    client: core_plugin::Entity,
    id: String,
    history: bool,
    state: BackendStateDto,
    histories: Vec<HistoryRead>,
) {
    let operation_id = operation_id();
    world
        .get_resource_mut::<PendingMclCommandResponses>()
        .unwrap()
        .snapshots
        .push(PendingSnapshot {
            operation_id: operation_id.clone(),
            id,
            connection_id,
            client,
            history,
        });
    spawn_snapshot(world, operation_id, state, histories);
}

fn report_backend_state(world: &mut World, targets: &[WebSocketMessageTarget]) {
    let completed: Vec<_> = world
        .event_reader::<DtoHistorySnapshotCompleted>()
        .into_iter()
        .cloned()
        .collect();
    for completed in completed {
        let backend = world
            .get_resource::<BackendStateReportCache>()
            .unwrap()
            .pending
            .as_ref()
            == Some(&completed.id);
        let result = completed.result.and_then(|value| {
            serde_json::from_value::<BackendStateDto>(value).map_err(|error| error.to_string())
        });
        let live = result
            .as_ref()
            .map(|state| {
                state.agents.iter().all(|metadata| {
                    world
                        .query_with::<Agent>()
                        .result()
                        .into_iter()
                        .any(|entity| {
                            world.get_component::<Agent>(entity).is_some_and(|agent| {
                                agent.resourceid.to_string() == metadata.agent.0
                                    && format!("{entity:?}") == metadata.agent_instance
                            })
                        })
                })
            })
            .unwrap_or(true);
        if backend {
            world
                .get_resource_mut::<BackendStateReportCache>()
                .unwrap()
                .pending = None;
            if !live {
                world
                    .get_resource_mut::<BackendStateReportCache>()
                    .unwrap()
                    .dirty = true;
                continue;
            }
            publish_backend_state(world, targets, result);
        } else {
            let pending = world
                .get_resource_mut::<PendingMclCommandResponses>()
                .unwrap();
            let Some(index) = pending
                .snapshots
                .iter()
                .position(|pending| pending.operation_id == completed.id)
            else {
                continue;
            };
            let pending = pending.snapshots.remove(index);
            if !live {
                continue;
            }
            if !world
                .client_by_connection(pending.connection_id)
                .is_some_and(|(entity, _)| entity == pending.client)
            {
                continue;
            }
            let message = match result {
                Ok(mut state) if pending.history => match state.histories.pop() {
                    Some(history) => ServerMessage::HistoryResult {
                        id: pending.id,
                        history,
                    },
                    None => continue,
                },
                Ok(state) => ServerMessage::StateSync { state },
                Err(error) => ServerMessage::RequestFailed {
                    id: pending.id,
                    error: ProtocolErrorDto::new("HistoryReadFailed", error),
                },
            };
            send_to_connection(world, pending.connection_id, &message);
        }
    }
    if world
        .get_resource::<BackendStateReportCache>()
        .unwrap()
        .pending
        .is_some()
    {
        return;
    }
    let prepared = BackendStateDto::from_domain((), &*world)
        .and_then(|state| Ok((state, prepare_histories(world, None)?)));
    match prepared {
        Ok((state, histories)) => {
            let cache = world.get_resource_mut::<BackendStateReportCache>().unwrap();
            if cache.prepared_state.as_ref() == Some(&state) && !cache.dirty {
                if let Some(cached) = cache.state.clone() {
                    publish_backend_state(world, targets, Ok(cached));
                }
                return;
            }
            cache.prepared_state = Some(state.clone());
            cache.dirty = false;
            if histories.is_empty() {
                publish_backend_state(world, targets, Ok(state));
                return;
            }
            let id = operation_id();
            world
                .get_resource_mut::<BackendStateReportCache>()
                .unwrap()
                .pending = Some(id.clone());
            spawn_snapshot(world, id, state, histories);
        }
        Err(error) => publish_backend_state(world, targets, Err(error.to_string())),
    }
}

fn publish_backend_state(
    world: &mut World,
    targets: &[WebSocketMessageTarget],
    result: Result<BackendStateDto, String>,
) {
    let state = match result {
        Ok(state) => state,
        Err(error) => {
            let error = error.to_string();
            let should_log = {
                let cache = world
                    .get_resource_mut::<BackendStateReportCache>()
                    .expect("DtoPlugin backend state report cache is missing");
                if cache.last_error.as_deref() == Some(&error) {
                    false
                } else {
                    cache.last_error = Some(error.clone());
                    true
                }
            };
            if should_log {
                tracing::warn!(error, "backend state report failed");
            }
            return;
        }
    };
    let mut recipients = world
        .get_resource::<WebSocketConnections>()
        .map(|connections| target_recipients(connections, targets))
        .unwrap_or_default();
    recipients.sort_unstable();
    recipients.dedup();
    let should_sync = {
        let cache = world
            .get_resource_mut::<BackendStateReportCache>()
            .expect("DtoPlugin backend state report cache is missing");
        cache.last_error = None;
        if cache.state.as_ref() == Some(&state) && cache.recipients == recipients {
            false
        } else {
            cache.state = Some(state.clone());
            cache.recipients = recipients;
            true
        }
    };
    if !should_sync {
        return;
    }
    send_to_targets(world, targets, ServerMessage::StateSync { state });
}

fn send_to_targets(world: &World, targets: &[WebSocketMessageTarget], message: ServerMessage) {
    let Ok(message) = serde_json::to_value(message) else {
        return;
    };
    for target in targets {
        world.send_event(WebSocketMessageSend {
            target: target.clone(),
            message: message.clone(),
        });
    }
}

fn target_recipients(
    connections: &WebSocketConnections,
    targets: &[WebSocketMessageTarget],
) -> Vec<u64> {
    targets
        .iter()
        .flat_map(|target| match target {
            WebSocketMessageTarget::Broadcast => connections.get_all(),
            WebSocketMessageTarget::Type(connection_type) => {
                connections.get_by_type(connection_type)
            }
            WebSocketMessageTarget::Name(name) => {
                connections.get_by_name(name).into_iter().collect()
            }
        })
        .map(|sender| sender.connection_id().get())
        .collect()
}

pub(crate) async fn forward_logs(
    stream: TracingStream,
    events: RuntimeEventSender,
    targets: Vec<WebSocketMessageTarget>,
) {
    let mut subscription = stream.subscribe();
    loop {
        let record = match subscription.recv().await {
            Ok(record) => record,
            Err(TracingStreamError::Lagged(_)) => continue,
            Err(TracingStreamError::Closed) | Err(_) => break,
        };
        let Ok(record): Result<LogRecordDto, _> = record.into_dto(()) else {
            continue;
        };
        let Ok(message) = serde_json::to_value(ServerMessage::Log { record }) else {
            continue;
        };
        for target in &targets {
            events.send_event(WebSocketMessageSend {
                target: target.clone(),
                message: message.clone(),
            });
        }
    }
}

#[cfg(test)]
mod response_tests {
    use super::{is_user_injection, response_is_live};

    #[test]
    fn busy_user_injection_detection_handles_nested_messages_without_hiding_admin_arrays() {
        assert!(is_user_injection(
            "INJECT ? TO entry.pending[-1]",
            &[
                serde_json::json!([{ "type": "inject", "messages": [{ "type": "user", "content": "hello" }] }])
            ]
        ));
        assert!(!is_user_injection(
            "INJECT ? TO session.visible",
            &[serde_json::json!(["tool_alias"])]
        ));
        assert!(!is_user_injection(
            "GET entry.pending",
            &[serde_json::json!([{ "type": "user", "content": "hello" }])]
        ));
        assert!(!is_user_injection(
            "INJECT ? TO entry.pending[-1]",
            &[serde_json::json!([{ "type": "assistant", "content": "user text" }])]
        ));
    }
    use core_plugin::World;
    use std::collections::HashSet;

    #[test]
    fn disconnected_mcl_start_and_stop_replies_are_purged_without_matching_new_client() {
        let mut world = World::default();
        let old = world.spawn();
        let mut mcl = vec![(1u64, old)];
        let mut starts = vec![(1u64, old)];
        let mut stops = vec![(1u64, old)];
        world.despawn(old);
        let current = world.spawn();
        let live = HashSet::from([(2u64, current)]);
        for replies in [&mut mcl, &mut starts, &mut stops] {
            replies.retain(|(connection, client)| response_is_live(&live, *connection, *client));
            assert!(replies.is_empty());
        }
        assert!(!response_is_live(&live, 1, old));
        assert!(!response_is_live(&HashSet::from([(1u64, current)]), 1, old));
        assert!(response_is_live(&live, 2, current));
        assert!(world.is_alive(current));
    }
}
