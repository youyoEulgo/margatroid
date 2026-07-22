pub mod error;
pub mod factory;
pub mod handlers;
pub mod human;
pub mod state;

use anyhow::{Context, Result};
use axum::{
    Router,
    routing::{get, post},
};
use state::AppState;
use tower_http::cors::CorsLayer;

/// 启动 HTTP 服务
///
/// 绑定到 AppState 中配置的地址并开始监听。
/// 调用方负责创建 AppState（可预先注册 workspace）。
pub async fn serve(state: AppState) -> Result<()> {
    let server_cfg = {
        let mgr = state.config_mgr.lock().await;
        mgr.app_config().server.clone()
    };

    let human_routes = Router::new()
        .route("/api/human/request", post(human::create_request))
        .route("/api/human/request/{id}", get(human::wait_reply))
        .route("/api/human/requests", get(human::list_requests))
        .route("/api/human/request/{id}/reply", post(human::submit_reply))
        .with_state(state.clone());

    let workspace_routes = Router::new()
        .route("/workspace", get(handlers::workspace::list))
        .route("/workspace/{name}/chat", post(handlers::workspace::chat))
        .route("/workspace/{name}/status", get(handlers::workspace::status))
        .route("/workspace/{name}/tasks", get(handlers::workspace::tasks))
        .route("/workspace/{name}/recent", get(handlers::workspace::recent))
        .route(
            "/workspace/{name}/conversation",
            get(handlers::workspace::conversation),
        )
        .route(
            "/workspace/{name}/events/{task_id}",
            get(handlers::workspace::events),
        )
        .route("/workspace/{name}/stream", get(handlers::workspace::stream))
        .with_state(state.clone());

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/chat", post(handlers::chat::chat))
        .route("/v1/stream", post(handlers::stream::stream))
        .route("/v1/providers", get(handlers::providers::list))
        .route("/admin/reload", post(handlers::admin::reload))
        .merge(workspace_routes)
        .merge(human_routes)
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", server_cfg.host, server_cfg.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {}", addr))?;
    tracing::info!("Margatroid Server listening on http://{}", addr);
    axum::serve(listener, app).await.context("serve exited")?;
    Ok(())
}
