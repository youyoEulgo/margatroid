use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Plugin, Resource};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../../ui/dist"]
struct UiAssets;

pub struct UiPlugin {
    schedule: String,
}

impl UiPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.to_owned(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn router() -> Router {
        Router::new()
            .route("/", get(index_handler))
            .route("/{*path}", get(asset_handler))
    }
}

impl Default for UiPlugin {
    fn default() -> Self {
        Self::new()
    }
}

pub struct UiPluginInstalled;

impl Resource for UiPluginInstalled {}

impl Plugin for UiPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<UiPluginInstalled>() {
            panic!("UiPlugin is already installed");
        }
        app.world_mut().insert_resource(UiPluginInstalled);
    }
}

async fn index_handler() -> Result<Response<Body>, StatusCode> {
    asset_response("index.html")
}

async fn asset_handler(Path(path): Path<String>) -> Result<Response<Body>, StatusCode> {
    asset_response(&path)
}

fn asset_response(path: &str) -> Result<Response<Body>, StatusCode> {
    let asset = UiAssets::get(path).ok_or(StatusCode::NOT_FOUND)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(path))
        .body(Body::from(asset.data.into_owned()))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}
