mod error;
mod events;
mod handler;
mod system;
mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AppAsyncExt, AsyncRuntimeHandle};
use core_plugin::{App, Plugin, Resource};

pub use error::{InferenceError, InferenceErrorKind};
pub use events::*;
pub use handler::WorldInferenceExt;
pub use types::{
    AgentInferenceSnapshot, ConfiguredModelRoute, DeepSeekAdapterFactory, ErasedProviderAdapter,
    ErasedProviderAdapterFactory, InferenceParameters, ModelId, ModelRoutesReloaded,
    OpenAiAdapterFactory, ProviderAdapter, ProviderAdapterFactory, ProviderHttpRequest,
    ProviderInferenceResponse, ProviderInput, ProviderResponseAccumulator, ProviderRouteInput,
    ProviderStreamDelta, StopReason, WorkspaceModelRoutes, WorkspaceModelRoutesRegistry,
};

use crate::handler::{
    default_config_path, load_model_routes, summarize_reqwest_error, InFlightInferences,
};
use crate::system::{
    cancel_inference_system, execute_prepared_inference, prepare_inference_system,
    publish_inference_output_system, reload_model_routes_system,
};
pub struct GlobalModelRoutes {
    path: PathBuf,
    pub(crate) factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>,
    routes: HashMap<ModelId, ConfiguredModelRoute>,
}

impl GlobalModelRoutes {
    pub(crate) fn load(
        path: PathBuf,
        factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>,
    ) -> Result<Self, InferenceError> {
        let routes = load_model_routes(&path, &factories)?;
        Ok(Self {
            path,
            factories,
            routes,
        })
    }

    pub(crate) fn reload(&mut self) -> Result<usize, InferenceError> {
        let routes = load_model_routes(&self.path, &self.factories)?;
        let count = routes.len();
        self.routes = routes;
        Ok(count)
    }

    pub(crate) fn get(&self, id: &ModelId) -> Option<ConfiguredModelRoute> {
        self.routes.get(id).cloned()
    }
}

impl Resource for GlobalModelRoutes {}

pub struct InferenceHttpClient {
    pub(crate) client: reqwest::Client,
}

impl InferenceHttpClient {
    pub(crate) fn new() -> Result<Self, InferenceError> {
        reqwest::Client::builder()
            .build()
            .map(|client| Self { client })
            .map_err(|error| {
                InferenceError::new(
                    InferenceErrorKind::RequestBuildFailed,
                    format!(
                        "inference HTTP client could not be created: {}",
                        summarize_reqwest_error(&error)
                    ),
                )
            })
    }
}

impl Resource for InferenceHttpClient {}

pub struct InferencePlugin {
    schedule: String,
    config_path: PathBuf,
    adapter_factories: HashMap<String, ErasedProviderAdapterFactory>,
}

impl InferencePlugin {
    pub fn new() -> Self {
        let mut adapter_factories: HashMap<String, ErasedProviderAdapterFactory> = HashMap::new();
        adapter_factories.insert("openai".into(), Arc::new(OpenAiAdapterFactory));
        adapter_factories.insert("deepseek".into(), Arc::new(DeepSeekAdapterFactory));
        Self {
            schedule: RuntimePlugin::PRE_UPDATE.to_owned(),
            config_path: default_config_path(),
            adapter_factories,
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

    pub fn with_api_type<Factory>(mut self, api_type: impl Into<String>, factory: Factory) -> Self
    where
        Factory: ProviderAdapterFactory,
    {
        let api_type = api_type.into();
        if api_type.is_empty() || self.adapter_factories.contains_key(&api_type) {
            InferenceError::new(
                InferenceErrorKind::UnsupportedApiType,
                "api type is empty or already registered",
            )
            .panic();
        }
        self.adapter_factories.insert(api_type, Arc::new(factory));
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
        if !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
        {
            InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "RuntimePlugin and AsyncRuntimePlugin are required",
            )
            .panic();
        }
        if app.world().contains_resource::<GlobalModelRoutes>()
            || app.world().contains_resource::<InferenceHttpClient>()
        {
            InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "InferencePlugin is already installed",
            )
            .panic();
        }
        if !app.contains_schedule(&self.schedule) {
            InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "InferencePlugin schedule does not exist",
            )
            .panic();
        }
        let factories = Arc::new(self.adapter_factories);
        let routes = GlobalModelRoutes::load(self.config_path, Arc::clone(&factories))
            .unwrap_or_else(|error| error.panic());
        let client = InferenceHttpClient::new().unwrap_or_else(|error| error.panic());
        let schedule = self.schedule;
        app.world_mut().insert_resource(routes);
        app.world_mut().insert_resource(client);
        app.world_mut()
            .insert_resource(WorkspaceModelRoutesRegistry::default());
        app.world_mut()
            .insert_resource(InFlightInferences::default());
        app.add_system(&schedule, reload_model_routes_system)
            .add_system(&schedule, cancel_inference_system)
            .add_system(&schedule, prepare_inference_system)
            .add_async_system(&schedule, execute_prepared_inference)
            .add_system(&schedule, publish_inference_output_system);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
    use async_runtime_plugin::AsyncRuntimePlugin;
    use margatroid_types::{AgentFailure, AgentFailureKind, AgentMessage, Message, ResourceId};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn inference_plugin_loads_routes_and_rejects_missing_snapshots() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("models.toml");
        fs::write(
            &path,
            r#"[[models]]
id = "test"
model = "test-model"
base_url = "https://example.test/v1"
api_key = "secret"
api_type = "openai"
"#,
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(InferencePlugin::default().with_config_path(path));
        let agent = app.world_mut().spawn();
        app.world().send_event(InferenceRequestEvent {
            id: "request".into(),
            agent,
            agent_id: ResourceId::parse("agent:test/agent0").unwrap(),
            messages: vec![Message::User {
                content: "hello".into(),
            }],
            tools: Vec::new(),
        });
        app.tick();
        app.tick();
        let failure = app
            .world()
            .event_reader::<AgentFailure>()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.id, "request");
        assert_eq!(failure.kind, AgentFailureKind::Inference);
        assert_eq!(
            failure.message,
            "InferenceSnapshotMissing: agent inference snapshot is missing"
        );

        app.world().send_event(ContextCompactionInferenceRequest {
            id: "compact".into(),
            agent,
            agent_id: ResourceId::parse("agent:test/agent0").unwrap(),
            messages: vec![Message::User {
                content: "summarize".into(),
            }],
        });
        app.tick();
        app.tick();
        let response = app
            .world()
            .event_reader::<ContextCompactionInferenceResponse>()
            .into_iter()
            .find(|event| event.id == "compact")
            .unwrap();
        assert_eq!(
            response.result.as_ref().unwrap_err().kind(),
            InferenceErrorKind::InferenceSnapshotMissing
        );
        assert!(app
            .world()
            .event_reader::<AgentMessage>()
            .into_iter()
            .all(|message| message.id != "compact"));
    }
}
