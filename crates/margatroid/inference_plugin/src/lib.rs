mod error;
mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AsyncRuntimeHandle, AsyncTaskError, WorldAsyncExt};
use core_plugin::{App, Plugin, Resource};
use std::collections::HashMap;
use std::path::PathBuf;

pub use error::{InferenceError, InferenceErrorKind};
pub use margatroid_types::inference_events::{
    InferenceRequest, ReloadModelRoutes, ReloadModelRoutesResult,
};
pub use margatroid_types::inference_request_types::{DomainRequest, DomainRequestConfig};
pub use margatroid_types::message_types::InferenceResponse;
pub use types::formats::ApiFormat;
pub use types::{
    ConfiguredModelRoute, ErasedProviderAdapter, InferenceParameters, ModelId, ProviderAdapter,
    ProviderHttpRequest, ProviderInferenceResponse, ProviderMessage, ProviderResponseAccumulator,
    ProviderRouteInput, ProviderStreamDelta, StopReason, Vendor,
};

pub struct InferenceConfig {
    pub(crate) path: PathBuf,
    pub(crate) routes: HashMap<ModelId, ConfiguredModelRoute>,
    pub(crate) client: reqwest::Client,
    pub(crate) schedule: String,
    pub(crate) reload_id: Option<String>,
}
impl Resource for InferenceConfig {}
impl InferenceConfig {
    pub fn model_count(&self) -> usize {
        self.routes.len()
    }
    pub fn get(&self, id: &ModelId) -> Option<&ConfiguredModelRoute> {
        self.routes.get(id)
    }
}

fn runtime_catalog(
    routes: &HashMap<ModelId, ConfiguredModelRoute>,
) -> margatroid_types::model_components::ModelRuntimeCatalog {
    margatroid_types::model_components::ModelRuntimeCatalog::new(
        routes.values().map(|route| route.runtime_info.clone()),
    )
}

pub struct InferencePlugin {
    schedule: String,
    config_path: PathBuf,
}
impl InferencePlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::PRE_UPDATE.into(),
            config_path: handler::default_config_path(),
        }
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
    pub fn with_config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = path.into();
        self
    }
}
impl Default for InferencePlugin {
    fn default() -> Self {
        Self::new()
    }
}
impl Plugin for InferencePlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>()
                && app.world().contains_resource::<AsyncRuntimeHandle>(),
            "RuntimePlugin and AsyncRuntimePlugin required"
        );
        assert!(
            !app.world().contains_resource::<InferenceConfig>(),
            "InferencePlugin already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "inference schedule missing"
        );
        let routes =
            handler::load_model_routes(&self.config_path).unwrap_or_else(|e| panic!("{e}"));
        let client = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|_| panic!("HTTP client creation failed"));
        app.world()
            .send_async_closure(&self.schedule, || async { Ok::<(), AsyncTaskError>(()) });
        app.world_mut().insert_resource(runtime_catalog(&routes));
        app.world_mut().insert_resource(InferenceConfig {
            path: self.config_path,
            routes,
            client,
            schedule: self.schedule.clone(),
            reload_id: None,
        });
        app.add_system(&self.schedule, system::inference_system);
    }
}
