mod diagnostics;
mod error;
mod handler;
mod types;
use core_plugin::App;
pub use error::ComposeError;
pub use types::ApplicationOptions;
pub fn build_app(options: ApplicationOptions) -> Result<App, ComposeError> {
    let parts = handler::prepare(&options)
        .inspect_err(|error| diagnostics::startup_failure(&options, error))?;
    let mut app = App::new();
    handler::install(&mut app, parts)?;
    Ok(app)
}

pub async fn shutdown_app(app: &mut App) -> Result<(), ComposeError> {
    handler::shutdown_application(app).await
}
