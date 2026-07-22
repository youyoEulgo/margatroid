use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use app_runtime_plugin::AppRuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use config_plugin::ConfigPlugin;
use core_plugin::{App, Plugin};
use event_bus_plugin::EventBusPlugin;
use external_event_plugin::ExternalEventPlugin;
use http_server_plugin::HttpServerPlugin;
use llm_plugin::LlmPlugin;
use log_plugin::{LogPlugin, LogStreamOptions};
use sandbox_plugin::SandboxPlugin;
use server_plugin::{LogEndpointOptions, ServerPlugin};
use skill_plugin::SkillPlugin;

#[derive(Clone)]
pub struct MargatroidDaemonPlugins {
    bind_address: SocketAddr,
    log_endpoint_token: Option<String>,
}

impl std::fmt::Debug for MargatroidDaemonPlugins {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MargatroidDaemonPlugins")
            .field("bind_address", &self.bind_address)
            .field(
                "log_endpoint_token",
                &self.log_endpoint_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl MargatroidDaemonPlugins {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bind_address(mut self, address: SocketAddr) -> Self {
        self.bind_address = address;
        self
    }

    pub fn with_log_stream_bearer_token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        assert!(!token.is_empty(), "log stream bearer token cannot be empty");
        self.log_endpoint_token = Some(token);
        self
    }
}

impl Default for MargatroidDaemonPlugins {
    fn default() -> Self {
        Self {
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000),
            log_endpoint_token: None,
        }
    }
}

impl Plugin for MargatroidDaemonPlugins {
    fn build(&self, app: &mut App) {
        let mut log_plugin = LogPlugin::default();
        let mut server_plugin = ServerPlugin::default();
        if let Some(token) = &self.log_endpoint_token {
            log_plugin = log_plugin.with_stream(LogStreamOptions::default());
            server_plugin = server_plugin
                .with_log_stream_endpoint(LogEndpointOptions::bearer_token(token.clone()));
        }
        app.add_plugins(log_plugin)
            .add_plugins(AppRuntimePlugin)
            .add_plugins(ExternalEventPlugin)
            .add_plugins(AsyncRuntimePlugin::default())
            .add_plugins(HttpServerPlugin::bind(self.bind_address))
            .add_plugins(ConfigPlugin::default())
            .add_plugins(EventBusPlugin::default())
            .add_plugins(LlmPlugin::default())
            .add_plugins(SandboxPlugin::default())
            .add_plugins(SkillPlugin)
            .add_plugins(server_plugin);
    }
}

#[cfg(test)]
mod tests {
    use http_server_plugin::{HttpServerFailed, HttpServerHandle};

    use super::*;

    #[test]
    fn default_group_builds_and_starts() {
        let mut app = App::new();
        app.add_plugins(
            MargatroidDaemonPlugins::default().with_bind_address("127.0.0.1:0".parse().unwrap()),
        );
        let mut failure_reader = app.event_reader::<HttpServerFailed>();
        app.tick();

        let server = app.world().resource::<HttpServerHandle>().unwrap();
        let failures = app.world().read_events(&mut failure_reader);
        assert!(
            server.address().is_some(),
            "HTTP server failed to start: {failures:?}"
        );
    }

    #[test]
    fn debug_redacts_log_stream_token() {
        let plugins = MargatroidDaemonPlugins::default().with_log_stream_bearer_token("top-secret");
        let debug = format!("{plugins:?}");
        assert!(!debug.contains("top-secret"));
        assert!(debug.contains("[REDACTED]"));
    }
}
