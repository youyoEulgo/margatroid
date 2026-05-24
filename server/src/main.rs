#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let config_mgr = assets::Manager::bootstrap()?;
    let state = server::state::AppState::new(config_mgr).await?;
    server::serve(state).await
}
