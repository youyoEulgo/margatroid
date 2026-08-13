mod logs;
mod outbound;

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::WorldAsyncExt;
use config_plugin::MargatroidConfig;
pub use config_plugin::WebSocketMessageTarget;
use core_plugin::{App, Event, Plugin, Resource, World};
use log_plugin::TracingStream;
use margatroid_protocol::{BackendStateDto, ClientMessage, IntoDomain, ServerMessage};
use margatroid_types::AgentSkillRouteAction;
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

#[derive(Clone, Debug, PartialEq, Eq)]
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
        app.add_websocket_event_route(&self.websocket_path)
            .add_system(&self.schedule, outbound::collect_external_events_system)
            .add_system(&self.schedule, dto_route_system);
    }
}

fn dto_route_system(world: &mut World) {
    let received = world
        .event_reader::<WebSocketMessageReceived>()
        .into_iter()
        .collect::<Vec<_>>();
    for received in received {
        let WebSocketMessage::Text(text) = &received.message else {
            tracing::warn!(
                connection = received.connection_id.get(),
                "ignoring non-text API message"
            );
            continue;
        };
        let request = match serde_json::from_str::<ClientMessage>(text.as_str()) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(connection = received.connection_id.get(), error = %error, "ignoring invalid API request");
                continue;
            }
        };
        match request {
            ClientMessage::ConnectionRegister { id, message } => {
                tracing::info!(connection = received.connection_id.get(), request_id = %id, api_type = "connection.register", "API request received");
                match message.into_domain((id, received.connection_id)) {
                    Ok(event) => world.send_event(event),
                    Err(error) => tracing::warn!(
                        connection = received.connection_id.get(),
                        error = %error,
                        "invalid connection.register payload"
                    ),
                }
            }
            ClientMessage::WorkspaceStart { id, message } => {
                tracing::info!(connection = received.connection_id.get(), request_id = %id, api_type = "workspace.start", "API request received");
                match message.into_domain(id) {
                    Ok(event) => world.send_event(event),
                    Err(error) => tracing::warn!(
                        connection = received.connection_id.get(),
                        error = %error,
                        "invalid workspace.start payload"
                    ),
                }
            }
            ClientMessage::WorkspaceStop { id, message } => {
                tracing::info!(connection = received.connection_id.get(), request_id = %id, api_type = "workspace.stop", "API request received");
                match message.into_domain(id) {
                    Ok(event) => world.send_event(event),
                    Err(error) => tracing::warn!(
                        connection = received.connection_id.get(),
                        error = %error,
                        "invalid workspace.stop payload"
                    ),
                }
            }
            ClientMessage::AgentMessage { id, message } => {
                tracing::info!(connection = received.connection_id.get(), request_id = %id, api_type = "agent.message", "API request received");
                match message.into_domain(id) {
                    Ok(event) => world.send_event(event),
                    Err(error) => tracing::warn!(
                        connection = received.connection_id.get(),
                        error = %error,
                        "invalid agent.message payload"
                    ),
                }
            }
            ClientMessage::AgentSkillLoad { id, message } => {
                tracing::info!(connection = received.connection_id.get(), request_id = %id, api_type = "agent.skill.load", "API request received");
                match message.into_domain((id, AgentSkillRouteAction::Load)) {
                    Ok(event) => world.send_event(event),
                    Err(error) => {
                        tracing::warn!(connection = received.connection_id.get(), error = %error, "invalid agent.skill.load payload")
                    }
                }
            }
            ClientMessage::AgentSkillUnload { id, message } => {
                tracing::info!(connection = received.connection_id.get(), request_id = %id, api_type = "agent.skill.unload", "API request received");
                match message.into_domain((id, AgentSkillRouteAction::Unload)) {
                    Ok(event) => world.send_event(event),
                    Err(error) => {
                        tracing::warn!(connection = received.connection_id.get(), error = %error, "invalid agent.skill.unload payload")
                    }
                }
            }
            ClientMessage::AgentSkillUnloadAll { id, message } => {
                tracing::info!(connection = received.connection_id.get(), request_id = %id, api_type = "agent.skill.unload_all", "API request received");
                match message.into_domain((id, AgentSkillRouteAction::UnloadAll)) {
                    Ok(event) => world.send_event(event),
                    Err(error) => {
                        tracing::warn!(connection = received.connection_id.get(), error = %error, "invalid agent.skill.unload_all payload")
                    }
                }
            }
        }
    }

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
