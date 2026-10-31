mod error;
mod handler;
mod types;
use core_plugin::App;
pub use error::ComposeError;
pub use types::ApplicationOptions;
pub fn build_app(options: ApplicationOptions) -> Result<App, ComposeError> {
    let parts = handler::prepare(&options)?;
    let mut app = App::new();
    handler::install(&mut app, parts)?;
    Ok(app)
}
