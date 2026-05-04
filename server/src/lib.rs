pub mod error;
pub mod factory;
pub mod handlers;
pub mod state;

use anyhow::{Context, Result};
use axum::{
    Router,
    routing::{get, post},
};
use state::AppState;

/// 启动 HTTP 服务
///
/// 初始化配置、provider、路由，绑定地址并开始监听。
/// 这是 server crate 的公共入口，同时被 `server/src/main.rs`
/// 和 `cli/src/main.rs` 的 `margatroid serve` 命令使用。
pub async fn serve() -> Result<()> {
    let config_mgr = assets::Manager::bootstrap()?;

    let server_cfg = config_mgr.app_config().server.clone();
    let app_state = AppState::new(config_mgr).await?;

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/chat", post(handlers::chat::chat))
        .route("/v1/stream", post(handlers::stream::stream))
        .route("/v1/providers", get(handlers::providers::list))
        .route("/admin/reload", post(handlers::admin::reload))
        .with_state(app_state);

    let addr = format!("{}:{}", server_cfg.host, server_cfg.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {}", addr))?;
    tracing::info!("Margatroid Server listening on http://{}", addr);
    axum::serve(listener, app).await.context("serve exited")?;
    Ok(())
}
