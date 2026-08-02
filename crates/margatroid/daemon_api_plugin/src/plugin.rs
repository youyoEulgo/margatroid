use std::convert::Infallible;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use core_plugin::{App, Plugin};
use futures_util::stream;
use http_server_plugin::{HttpAppExt, HttpServerHandle};
use log_plugin::{LogStream, LogStreamError};

use crate::resource::{LogEndpointOptions, ServerPluginOptions};

#[derive(Clone, Debug, Default)]
pub struct DaemonApiPlugin {
    options: ServerPluginOptions,
}

impl DaemonApiPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(options: ServerPluginOptions) -> Self {
        Self { options }
    }

    pub fn with_log_stream_endpoint(mut self, options: LogEndpointOptions) -> Self {
        self.options.log_endpoint = Some(options);
        self
    }
}

impl Plugin for DaemonApiPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().resource::<HttpServerHandle>().is_some(),
            "HttpServerPlugin must be installed before DaemonApiPlugin"
        );
        app.add_http_routes(Router::new().route("/health", get(health)));

        if let Some(options) = &self.options.log_endpoint {
            let stream = app
                .world()
                .resource::<LogStream>()
                .unwrap_or_else(|| {
                    panic!("LogPlugin stream layer must be enabled before the log endpoint")
                })
                .clone();
            app.add_http_routes(log_routes(stream, options.clone()));
        }
    }
}

async fn health() -> &'static str {
    "ok"
}

#[derive(Clone)]
struct LogRouteState {
    stream: LogStream,
    authorization: String,
}

fn log_routes(stream: LogStream, options: LogEndpointOptions) -> Router {
    Router::new()
        .route("/v1/logs/stream", get(stream_logs))
        .with_state(LogRouteState {
            stream,
            authorization: options.authorization_header(),
        })
}

async fn stream_logs(
    State(state): State<LogRouteState>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>>, StatusCode> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if authorization != Some(state.authorization.as_str()) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let subscription = state.stream.subscribe();
    let stream = stream::unfold(subscription, |mut subscription| async move {
        match subscription.recv().await {
            Ok(record) => {
                let data = serde_json::to_string(&record).unwrap_or_else(|_| "{}".into());
                Some((
                    Ok(SseEvent::default().event("log").data(data)),
                    subscription,
                ))
            }
            Err(LogStreamError::Lagged(count)) => Some((
                Ok(SseEvent::default().event("lagged").data(count.to_string())),
                subscription,
            )),
            Err(LogStreamError::Closed) => None,
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use app_runtime_plugin::AppRuntimePlugin;
    use axum::body::Body;
    use axum::http::Request;
    use http_server_plugin::HttpServerPlugin;
    use log_plugin::{LogPlugin, LogStreamOptions};
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn registers_business_routes_on_http_server() {
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        app.add_plugins(HttpServerPlugin::bind("127.0.0.1:0"));
        app.add_plugins(DaemonApiPlugin::default());
        app.tick();

        assert!(app
            .world()
            .resource::<HttpServerHandle>()
            .unwrap()
            .address()
            .is_some());
    }

    #[test]
    fn log_endpoint_requires_stream_layer() {
        let result = std::panic::catch_unwind(|| {
            let mut app = App::new();
            app.add_plugins(AppRuntimePlugin);
            app.add_plugins(HttpServerPlugin::bind("127.0.0.1:0"));
            app.add_plugins(
                DaemonApiPlugin::default()
                    .with_log_stream_endpoint(LogEndpointOptions::bearer_token("test-token")),
            );
        });
        assert!(result.is_err());
    }

    #[test]
    fn log_endpoint_builds_with_stream_layer() {
        let mut app = App::new();
        app.add_plugins(LogPlugin::default().with_stream(LogStreamOptions::default()));
        app.add_plugins(AppRuntimePlugin);
        app.add_plugins(HttpServerPlugin::bind("127.0.0.1:0"));
        app.add_plugins(
            DaemonApiPlugin::default()
                .with_log_stream_endpoint(LogEndpointOptions::bearer_token("test-token")),
        );
    }

    #[tokio::test]
    async fn log_endpoint_requires_matching_bearer_token() {
        let mut app = App::new();
        app.add_plugins(LogPlugin::default().with_stream(LogStreamOptions::default()));
        let stream = app.world().resource::<LogStream>().unwrap().clone();
        let router = log_routes(stream, LogEndpointOptions::bearer_token("expected-token"));

        let unauthorized = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/logs/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = router
            .oneshot(
                Request::builder()
                    .uri("/v1/logs/stream")
                    .header(header::AUTHORIZATION, "Bearer expected-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
    }
}
