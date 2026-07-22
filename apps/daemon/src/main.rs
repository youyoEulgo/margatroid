use std::net::SocketAddr;

use app_runtime_plugin::AppRunExt;
use core_plugin::App;
use margatroid_defaults::MargatroidDaemonPlugins;

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:3939";

fn main() {
    let bind_address = bind_address();
    let mut plugins = MargatroidDaemonPlugins::default().with_bind_address(bind_address);
    if let Some(token) = non_empty_env("MARGATROID_LOG_TOKEN") {
        plugins = plugins.with_log_stream_bearer_token(token);
    }

    let mut app = App::new();
    app.add_plugins(plugins);
    tracing::info!(%bind_address, "margatroidd starting");
    app.run();
}

fn bind_address() -> SocketAddr {
    let value = std::env::var("MARGATROID_BIND").unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.into());
    value
        .parse()
        .unwrap_or_else(|error| panic!("invalid MARGATROID_BIND `{value}`: {error}"))
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bind_address_is_valid() {
        assert_eq!(
            DEFAULT_BIND_ADDRESS.parse::<SocketAddr>().unwrap(),
            "127.0.0.1:3939".parse().unwrap()
        );
    }
}
