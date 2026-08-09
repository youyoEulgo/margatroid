use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Plugin, Resource, World};
use server_plugin::{RegisterConnection, WebSocketConnections};

#[derive(Clone, Debug)]
pub struct ConnectionPlugin {
    schedule: String,
}

impl ConnectionPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for ConnectionPlugin {
    fn default() -> Self {
        Self::new()
    }
}

struct ConnectionPluginInstalled;

impl Resource for ConnectionPluginInstalled {}

impl Plugin for ConnectionPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<ConnectionPluginInstalled>() {
            panic!("ConnectionPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!(
                "ConnectionPlugin schedule does not exist: {}",
                self.schedule
            );
        }
        if !app.world().contains_resource::<WebSocketConnections>() {
            panic!("ServerPlugin must be installed before ConnectionPlugin");
        }
        app.world_mut().insert_resource(ConnectionPluginInstalled);
        app.add_system(&self.schedule, connection_registration_system);
    }
}

fn connection_registration_system(world: &mut World) {
    let requests = world
        .event_reader::<RegisterConnection>()
        .into_iter()
        .collect::<Vec<_>>();
    let Some(connections) = world.get_resource::<WebSocketConnections>().cloned() else {
        return;
    };
    for request in requests {
        let client_type = request.client_type.trim();
        if !valid_client_type(client_type) {
            tracing::warn!(connection = request.connection_id.get(), client_type = %request.client_type, "invalid WebSocket client type");
            continue;
        }
        let name = format!("{client_type}-{}", request.connection_id.get());
        if !connections.set_connection_type(request.connection_id, client_type) {
            tracing::warn!(
                connection = request.connection_id.get(),
                "WebSocket connection disappeared before registration"
            );
            continue;
        }
        if let Err(error) = connections.set_name(request.connection_id, name.clone()) {
            tracing::warn!(connection = request.connection_id.get(), error = %error, "WebSocket connection could not be named");
            continue;
        }
        tracing::info!(
            request_id = %request.id,
            connection = request.connection_id.get(),
            client_type,
            name,
            "WebSocket connection registered"
        );
    }
}

fn valid_client_type(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_type_uses_stable_identifier_characters() {
        assert!(valid_client_type("webui"));
        assert!(valid_client_type("desktop-2"));
        assert!(!valid_client_type(""));
        assert!(!valid_client_type("WebUI"));
        assert!(!valid_client_type("web ui"));
    }
}
