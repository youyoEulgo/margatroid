mod error;
mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AsyncRuntimeHandle, AsyncTaskError, WorldAsyncExt};
use core_plugin::{App, Plugin, Resource};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub use error::{InferenceError, InferenceErrorKind};
pub use margatroid_types::inference_events::{
    InferenceRequest, ReloadModelRoutes, ReloadModelRoutesResult,
};
pub use margatroid_types::message_types::InferenceResponse;
pub use types::{
    ConfiguredModelRoute, DeepSeekAdapterFactory, ErasedProviderAdapter,
    ErasedProviderAdapterFactory, InferenceParameters, ModelId, OpenAiAdapterFactory,
    ProviderAdapter, ProviderAdapterFactory, ProviderHttpRequest, ProviderInferenceResponse,
    ProviderInput, ProviderMessage, ProviderResponseAccumulator, ProviderRouteInput,
    ProviderStreamDelta, StopReason,
};

pub struct InferenceConfig {
    pub(crate) path: PathBuf,
    pub(crate) factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>,
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

pub struct InferencePlugin {
    schedule: String,
    config_path: PathBuf,
    factories: HashMap<String, ErasedProviderAdapterFactory>,
}
impl InferencePlugin {
    pub fn new() -> Self {
        let mut factories: HashMap<String, ErasedProviderAdapterFactory> = HashMap::new();
        factories.insert("openai".into(), Arc::new(OpenAiAdapterFactory));
        factories.insert("deepseek".into(), Arc::new(DeepSeekAdapterFactory));
        Self {
            schedule: RuntimePlugin::PRE_UPDATE.into(),
            config_path: handler::default_config_path(),
            factories,
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
    pub fn with_api_type<F: ProviderAdapterFactory>(
        mut self,
        api_type: impl Into<String>,
        factory: F,
    ) -> Self {
        let api_type = api_type.into();
        assert!(
            !api_type.trim().is_empty() && !self.factories.contains_key(&api_type),
            "api type empty or duplicated"
        );
        self.factories.insert(api_type, Arc::new(factory));
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
        let factories = Arc::new(self.factories);
        let routes = handler::load_model_routes(&self.config_path, &factories)
            .unwrap_or_else(|e| panic!("{e}"));
        let client = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|_| panic!("HTTP client creation failed"));
        app.world()
            .send_async_closure(&self.schedule, || async { Ok::<(), AsyncTaskError>(()) });
        app.world_mut().insert_resource(InferenceConfig {
            path: self.config_path,
            factories,
            routes,
            client,
            schedule: self.schedule.clone(),
            reload_id: None,
        });
        app.add_system(&self.schedule, system::inference_system);
    }
}
