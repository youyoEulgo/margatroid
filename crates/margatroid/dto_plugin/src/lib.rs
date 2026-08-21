mod logs;
mod outbound;

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::WorldAsyncExt;
use config_plugin::MargatroidConfig;
pub use config_plugin::WebSocketMessageTarget;
use core_plugin::{App, Event, Plugin, Resource, World};
use log_plugin::TracingStream;
use margatroid_protocol::{BackendStateDto, ClientMessage, IntoDomain, ServerMessage};
use server_plugin::{
    AppServerExt, WebSocketConnections, WebSocketMessage, WebSocketMessageReceived,
    WebSocketMessageSender,
};

#[derive(Clone, Debug)]
pub struct DtoPlugin {
    websocket_path: String,
    schedule: String,
}

impl DtoPlugin {
    pub fn new() -> Self {
        Self {
            websocket_path: "/ws".into(),
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }

    pub fn with_websocket_path(mut self, path: impl Into<String>) -> Self {
        self.websocket_path = path.into();
        self
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for DtoPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WebSocketMessageSend {
    pub target: WebSocketMessageTarget,
    pub message: ServerMessage,
}

impl Event for WebSocketMessageSend {}

struct DtoPluginInstalled;

impl Resource for DtoPluginInstalled {}

#[derive(Default)]
struct BackendStateReportCache {
    state: Option<BackendStateDto>,
    recipients: Vec<u64>,
    last_error: Option<String>,
}

impl Resource for BackendStateReportCache {}

#[derive(Default)]
struct PendingMclCommandResponses {
    commands: Vec<PendingMclCommandResponse>,
}

impl Resource for PendingMclCommandResponses {}

struct PendingMclCommandResponse {
    id: String,
    connection_id: server_plugin::WebSocketConnectionId,
    response: std::sync::Mutex<std::sync::mpsc::Receiver<Result<serde_json::Value, String>>>,
}

impl Plugin for DtoPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<DtoPluginInstalled>() {
            panic!("DtoPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("DtoPlugin schedule does not exist: {}", self.schedule);
        }
        if !app.world().contains_resource::<WebSocketConnections>() {
            panic!("ServerPlugin must be installed before DtoPlugin");
        }
        let targets = app
            .world()
            .get_resource::<MargatroidConfig>()
            .cloned()
            .expect("ConfigPlugin must be installed before DtoPlugin");
        let stream = app
            .world()
            .get_resource::<TracingStream>()
            .cloned()
            .expect("LogPlugin with a TracingStream must be installed before DtoPlugin");
        let events = app.world().event_sender();
        app.world().spawn_async_service(logs::forward_logs(
            stream,
            events,
            targets.logs().to_vec(),
        ));
        app.world_mut().insert_resource(DtoPluginInstalled);
        app.world_mut()
            .insert_resource(BackendStateReportCache::default());
        app.world_mut()
            .insert_resource(PendingMclCommandResponses::default());
        app.add_websocket_event_route(&self.websocket_path)
            .add_system(&self.schedule, outbound::collect_external_events_system)
            .add_system(&self.schedule, dto_route_system);
    }
}

fn dto_route_system(world: &mut World) {
    let received = world
        .event_reader::<WebSocketMessageReceived>()
        .into_iter()
        .map(|received| (received.connection_id, received.message.clone()))
        .collect::<Vec<_>>();
    for (connection_id, message) in received {
        let WebSocketMessage::Text(text) = &message else {
            tracing::warn!(
                connection = connection_id.get(),
                "ignoring non-text API message"
            );
            continue;
        };
        let request = match serde_json::from_str::<ClientMessage>(text.as_str()) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(connection = connection_id.get(), error = %error, "ignoring invalid API request");
                continue;
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
            ClientMessage::AgentWorkflowAttach { id, message } => {
                tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "agent.workflow.attach", "API request received");
                match message.into_domain(id) {
                    Ok(event) => world.send_event(event),
                    Err(error) => {
                        tracing::warn!(connection = connection_id.get(), error = %error, "invalid agent.workflow.attach payload")
                    }
                }
            }
            ClientMessage::AgentWorkflowDetach { id, message } => {
                tracing::info!(connection = connection_id.get(), request_id = %id, api_type = "agent.workflow.detach", "API request received");
                match message.into_domain(id) {
                    Ok(event) => world.send_event(event),
                    Err(error) => {
                        tracing::warn!(connection = connection_id.get(), error = %error, "invalid agent.workflow.detach payload")
                    }
                }
            }
        }
    }

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

    let outgoing = world
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .collect::<Vec<_>>();
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
