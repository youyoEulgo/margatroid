mod components;
mod error;
mod events;
mod types;
pub use components::{ActiveInference, AgentInferenceConfig};
use core_plugin::Resource;
pub use error::{InferenceError, InferenceErrorKind};
pub use events::{InferenceReply, InferenceRequest, InitInference};
use std::collections::BTreeMap;
pub use types::*;
#[derive(Clone, Default, Debug)]
pub struct ModelRoutes {
    entries: BTreeMap<String, ModelRoute>,
}
impl ModelRoutes {
    pub fn new(entries: BTreeMap<String, ModelRoute>) -> Self {
        Self { entries }
    }
    pub fn replace(
        &mut self,
        entries: BTreeMap<String, ModelRoute>,
        configs: &[AgentInferenceConfig],
    ) {
        self.entries = entries;
        for config in configs {
            config.publish_current(self);
        }
    }
    pub fn entries(&self) -> &BTreeMap<String, ModelRoute> {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&ModelRoute> {
        self.entries.get(id)
    }
}
impl Resource for ModelRoutes {}

pub use components::PublicModelInfo;
pub type ModelSyncBinding = client_core::ProbeFactory<Option<PublicModelInfo>>;

/// File operations supplied by the inference plugin; this resource never caches file contents.
#[derive(Clone)]
pub struct ModelConfigFile {
    path: std::path::PathBuf,
    read: std::sync::Arc<dyn Fn() -> ModelConfigFuture<String> + Send + Sync>,
    load: std::sync::Arc<dyn Fn() -> ModelConfigFuture<ModelRoutes> + Send + Sync>,
    replace: std::sync::Arc<dyn Fn(String) -> ModelConfigFuture<()> + Send + Sync>,
}
pub type ModelConfigFuture<T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, InferenceError>> + Send>>;
impl ModelConfigFile {
    pub fn new(
        path: std::path::PathBuf,
        read: impl Fn() -> ModelConfigFuture<String> + Send + Sync + 'static,
        load: impl Fn() -> ModelConfigFuture<ModelRoutes> + Send + Sync + 'static,
        replace: impl Fn(String) -> ModelConfigFuture<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            path,
            read: std::sync::Arc::new(read),
            load: std::sync::Arc::new(load),
            replace: std::sync::Arc::new(replace),
        }
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub async fn read(&self) -> Result<String, InferenceError> {
        (self.read)().await
    }
    pub async fn load(&self) -> Result<ModelRoutes, InferenceError> {
        (self.load)().await
    }
    pub async fn replace(&self, content: String) -> Result<(), InferenceError> {
        (self.replace)(content).await
    }
}
impl Resource for ModelConfigFile {}
