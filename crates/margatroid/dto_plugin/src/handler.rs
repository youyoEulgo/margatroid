use app_runtime_plugin::{RuntimeEventSender, WorldEventExt};
use config_plugin::{MargatroidConfig, WebSocketMessageTarget};
use core_plugin::World;
use log_plugin::{TracingStream, TracingStreamError};
use margatroid_protocol::{
    AgentFailureDto, AgentMessageDto, BackendStateDto, ClientMessage, IntoDomain, IntoDto,
    LogRecordDto, MessageDto, ProtocolErrorKind, ServerMessage, WorkspaceInfoDto,
};
use margatroid_types::{AgentFailure, AgentMessage};
use server_plugin::{
    ServerFailed, ServerStarted, ServerStopped, WebSocketConnected, WebSocketConnectionId,
    WebSocketConnections, WebSocketDisconnected, WebSocketMessage, WebSocketMessageSender,
    WebSocketProtocolFailed,
};
use workspace_plugin::{StartWorkspaceResult, StopWorkspaceByReferenceResult};

use crate::types::{
    BackendStateReportCache, PendingMclCommandResponse, PendingMclCommandResponses,
};
use crate::WebSocketMessageSend;

pub(crate) fn handle_inbound_message(
    world: &mut World,
    connection_id: WebSocketConnectionId,
    message: WebSocketMessage,
) {
    let WebSocketMessage::Text(text) = &message else {
        tracing::warn!(
            connection = connection_id.get(),
            "ignoring non-text API message"
        );
        return;
    };
    let request = match serde_json::from_str::<ClientMessage>(text.as_str()) {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(connection = connection_id.get(), error = %error, "ignoring invalid API request");
            return;
        }
    };
    match request {
        ClientMessage::ConnectionRegister { id, message } => {
            tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "connection.register", "API request received");
            match message.into_domain((id, connection_id)) {
                Ok(event) => world.send_event(event),
                Err(error) => tracing::warn!(
                    connection = connection_id.get(),
                    error = %error,
                    "invalid connection.register payload"
                ),
            }
        }
        ClientMessage::WorkspaceStart { id, message } => {
            tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "workspace.start", "API request received");
            match message.into_domain(id) {
                Ok(event) => world.send_event(event),
                Err(error) => tracing::warn!(
                    connection = connection_id.get(),
                    error = %error,
                    "invalid workspace.start payload"
                ),
            }
        }
        ClientMessage::WorkspaceStop { id, message } => {
            tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "workspace.stop", "API request received");
            match message.into_domain(id) {
                Ok(event) => world.send_event(event),
                Err(error) => tracing::warn!(
                    connection = connection_id.get(),
                    error = %error,
                    "invalid workspace.stop payload"
                ),
            }
        }
        ClientMessage::AgentMessage { id, message } => {
            tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "agent.message", "API request received");
            match message.into_domain(id) {
                Ok(event) => world.send_event(event),
                Err(error) => tracing::warn!(
                    connection = connection_id.get(),
                    error = %error,
                    "invalid agent.message payload"
                ),
            }
        }
        ClientMessage::AgentAssistant { id, message } => {
            tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "agent.assistant", "API request received");
            match message.into_domain(id) {
                Ok(event) => world.send_event(event),
                Err(error) => tracing::warn!(
                    connection = connection_id.get(),
                    error = %error,
                    "invalid agent.assistant payload"
                ),
            }
        }
        ClientMessage::MclCommand { id, message } => {
            tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "mcl.command", "API request received");
            let (reply, response) = std::sync::mpsc::channel();
            match message.into_domain((id.clone(), reply)) {
                Ok(event) => {
                    world.send_event(event);
                    world
                        .get_resource_mut::<PendingMclCommandResponses>()
                        .expect("DtoPlugin MCL command responses are missing")
                        .commands
                        .push(PendingMclCommandResponse {
                            id,
                            connection_id,
                            response: std::sync::Mutex::new(response),
                        });
                }
                Err(error) => {
                    let response = ServerMessage::MclCommandResult {
                        id,
                        result: Err(error.to_string()),
                    };
                    if let (Some(sender), Ok(encoded)) = (
                        world
                            .get_resource::<WebSocketConnections>()
                            .and_then(|connections| connections.get(connection_id)),
                        serde_json::to_string(&response),
                    ) {
                        let _ = sender.try_send(WebSocketMessage::Text(encoded.into()));
                    }
                }
            }
        }
        ClientMessage::AgentTurnAbort { id, message } => {
            tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "agent.turn.abort", "API request received");
            match message.into_domain(id) {
                Ok(event) => world.send_event(event),
                Err(error) => tracing::warn!(
                    connection = connection_id.get(),
                    error = %error,
                    "invalid agent.turn.abort payload"
                ),
            }
        }
    }
}

pub(crate) fn handle_pending_mcl_responses(world: &mut World) {
    let connections = world.get_resource::<WebSocketConnections>().cloned();
    world
        .get_resource_mut::<PendingMclCommandResponses>()
        .expect("DtoPlugin MCL command responses are missing")
        .commands
        .retain(|pending| {
            match pending
                .response
                .lock()
                .expect("MCL command response lock poisoned")
                .try_recv()
            {
                Ok(result) => {
                    if let Some(sender) = connections
                        .as_ref()
                        .and_then(|connections| connections.get(pending.connection_id))
                    {
                        let message = ServerMessage::MclCommandResult {
                            id: pending.id.clone(),
                            result,
                        };
                        if let Ok(encoded) = serde_json::to_string(&message) {
                            let _ = sender.try_send(WebSocketMessage::Text(encoded.into()));
                        }
                    }
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
            }
        });
}

pub(crate) fn handle_outbound_messages(world: &mut World, outgoing: Vec<WebSocketMessageSend>) {
    let Some(connections) = world.get_resource::<WebSocketConnections>().cloned() else {
        return;
    };
    for outgoing in outgoing {
        let is_log = matches!(&outgoing.message, ServerMessage::Log { .. });
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

pub(crate) fn handle_collect_external_events(world: &mut World) {
    let targets = world
        .get_resource::<MargatroidConfig>()
        .cloned()
        .expect("ConfigPlugin must be installed before DtoPlugin");
    report_server_events(world);
    report_workspace_events(world, targets.logs());
    report_workspace_stop_events(world, targets.logs());
    report_agent_messages(world, targets.member_messages());
    report_agent_failures(world, targets.logs());
    report_backend_state(world, targets.backend_state());
}

fn report_workspace_stop_events(world: &World, targets: &[WebSocketMessageTarget]) {
    for result in world.event_reader::<StopWorkspaceByReferenceResult>() {
        let message = match &result.result {
            Ok(()) => {
                tracing::info!(request_id = %result.id, workspace = %result.workspace.name, project_root = %result.workspace.project_root.display(), "workspace stopped");
                ServerMessage::WorkspaceStopped {
                    id: result.id.clone(),
                    workspace: result
                        .workspace
                        .clone()
                        .into_dto(())
                        .expect("workspace reference conversion cannot fail"),
                }
            }
            Err(error) => {
                tracing::error!(request_id = %result.id, workspace = %result.workspace.name, project_root = %result.workspace.project_root.display(), error = %error, "workspace stop failed");
                ServerMessage::WorkspaceStopFailed {
                    id: result.id.clone(),
                    error: error.to_string(),
                }
            }
        };
        send_to_targets(world, targets, message);
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

fn report_backend_state(world: &mut World, targets: &[WebSocketMessageTarget]) {
    let state: BackendStateDto = match ().into_dto(&*world) {
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

fn report_workspace_events(world: &World, targets: &[WebSocketMessageTarget]) {
    for result in world.event_reader::<StartWorkspaceResult>() {
        match &result.result {
            Ok(workspace) => match IntoDto::<WorkspaceInfoDto, _>::into_dto(*workspace, world) {
                Ok(info) => {
                    tracing::info!(request_id = %result.id, workspace = %info.name, project_root = %info.project_root, manager = %info.manager, agents = info.agents.len(), "workspace started");
                    send_to_targets(
                        world,
                        targets,
                        ServerMessage::WorkspaceStarted {
                            id: result.id.clone(),
                            workspace: info,
                        },
                    );
                }
                Err(error) => tracing::warn!(
                    request_id = %result.id,
                    error = %error,
                    "workspace DTO conversion failed"
                ),
            },
            Err(error) => {
                tracing::error!(request_id = %result.id, error = %error, "workspace start failed");
                send_to_targets(
                    world,
                    targets,
                    ServerMessage::WorkspaceStartFailed {
                        id: result.id.clone(),
                        error: error.to_string(),
                    },
                );
            }
        }
    }
}

fn report_agent_messages(world: &World, targets: &[WebSocketMessageTarget]) {
    let messages = world
        .event_reader::<AgentMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for message in messages {
        let message: AgentMessageDto = match (&message).into_dto(world) {
            Ok(message) => message,
            Err(error) if error.kind() == ProtocolErrorKind::UnsupportedMessage => continue,
            Err(error) => {
                tracing::warn!(request_id = %message.id, error = %error, "Agent message DTO conversion failed");
                continue;
            }
        };
        match &message.message {
            MessageDto::User { .. } => tracing::info!(
                request_id = %message.id,
                workspace = %message.workspace.name,
                agent = %message.agent,
                "user message sent"
            ),
            MessageDto::Assistant { tool_calls, .. } => tracing::info!(
                request_id = %message.id,
                workspace = %message.workspace.name,
                agent = %message.agent,
                tool_calls = tool_calls.len(),
                "assistant message sent"
            ),
            MessageDto::Tool { .. } | MessageDto::Error { .. } => continue,
        }
        send_to_targets(world, targets, ServerMessage::AgentMessage { message });
    }
}

fn report_agent_failures(world: &World, targets: &[WebSocketMessageTarget]) {
    let failures = world
        .event_reader::<AgentFailure>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for failure in failures {
        let dto: AgentFailureDto = match (&failure).into_dto(world) {
            Ok(dto) => dto,
            Err(error) => {
                tracing::warn!(request_id = %failure.id, error = %error, "Agent failure DTO conversion failed");
                continue;
            }
        };
        tracing::warn!(
            request_id = %failure.id,
            agent = %dto.agent,
            kind = %dto.kind,
            error = %failure.message,
            "agent turn failed"
        );
        send_to_targets(world, targets, ServerMessage::AgentFailure { failure: dto });
    }
}

fn send_to_targets(world: &World, targets: &[WebSocketMessageTarget], message: ServerMessage) {
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
        for target in &targets {
            events.send_event(WebSocketMessageSend {
                target: target.clone(),
                message: ServerMessage::Log {
                    record: record.clone(),
                },
            });
        }
    }
}
