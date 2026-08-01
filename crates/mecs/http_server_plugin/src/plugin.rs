use std::net::ToSocketAddrs;
use std::time::Duration;

use app_runtime_plugin::{AppControl, AppShutdownExt};
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::Router;
use core_plugin::{App, Plugin, Stage, World};
use tower_http::timeout::TimeoutLayer;

use crate::events::HttpServerFailed;
use crate::options::HttpServerOptions;
use crate::resource::{HttpRoutes, HttpServerHandle};

#[derive(Clone, Debug, Default)]
pub struct HttpServerPlugin {
    options: HttpServerOptions,
}

impl HttpServerPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(address: impl ToSocketAddrs) -> Self {
        let address = address
            .to_socket_addrs()
            .unwrap_or_else(|error| panic!("invalid HTTP bind address: {error}"))
            .next()
            .expect("HTTP bind address resolved to no socket addresses");
        Self {
            options: HttpServerOptions::bind(address),
        }
    }

    pub fn with_options(options: HttpServerOptions) -> Self {
        Self { options }
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.options = self.options.with_request_timeout(timeout);
        self
    }

    pub fn with_max_body_size(mut self, bytes: usize) -> Self {
        self.options = self.options.with_max_body_size(bytes);
        self
    }

    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.options = self.options.with_shutdown_timeout(timeout);
        self
    }
}

impl Plugin for HttpServerPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().resource::<AppControl>().is_some(),
            "AppRuntimePlugin must be installed before HttpServerPlugin"
        );
        assert!(
            app.world().resource::<HttpRoutes>().is_none(),
            "HttpServerPlugin can only be installed once"
        );
        app.add_event::<HttpServerFailed>();
        app.add_resource(HttpRoutes::new());
        app.add_resource(self.options.clone());
        app.add_resource(HttpServerHandle::new());
        let server = app
            .world()
            .resource::<HttpServerHandle>()
            .expect("HttpServerHandle should be registered")
            .clone();
        app.on_shutdown(move |_world| {
            server.shutdown();
        });

        app.add_systems(Stage::Startup, [start_http_server]);
    }
}

pub trait HttpAppExt {
    fn add_http_routes(&mut self, router: Router) -> &mut Self;
}

impl HttpAppExt for App {
    fn add_http_routes(&mut self, router: Router) -> &mut Self {
        self.world()
            .resource::<HttpRoutes>()
            .unwrap_or_else(|| {
                panic!("HttpServerPlugin must be installed before registering HTTP routes")
            })
            .merge(router);
        self
    }
}

fn start_http_server(world: &mut World) {
    let options = world
        .resource::<HttpServerOptions>()
        .expect("HttpServerOptions should be registered")
        .clone();
    let router = world
        .resource::<HttpRoutes>()
        .expect("HttpRoutes should be registered")
        .snapshot()
        .layer(DefaultBodyLimit::max(options.max_body_size()))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            options.request_timeout(),
        ));
    let result = world
        .resource::<HttpServerHandle>()
        .expect("HttpServerHandle should be registered")
        .start(options, router);
    if let Err(error) = result {
        world.emit_event(HttpServerFailed {
            message: error.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};

    use app_runtime_plugin::AppRuntimePlugin;
    use axum::routing::get;

    use super::*;

    #[test]
    fn starts_axum_and_serves_registered_route() {
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        app.add_plugins(
            HttpServerPlugin::bind("127.0.0.1:0").with_shutdown_timeout(Duration::from_millis(20)),
        );
        app.add_http_routes(Router::new().route("/health", get(|| async { "ok" })));
        app.tick();

        std::thread::sleep(Duration::from_millis(40));

        let address = app
            .world()
            .resource::<HttpServerHandle>()
            .unwrap()
            .address()
            .unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(b"GET /health HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.ends_with("ok"), "{response}");
    }

    #[test]
    fn reports_bind_failure_as_event() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        app.add_plugins(HttpServerPlugin::bind(address));
        let mut reader = app.event_reader::<HttpServerFailed>();
        app.tick();

        assert_eq!(app.world().read_events(&mut reader).len(), 1);
    }
}
