use core_plugin::Resource;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRuntimeInfo {
    pub id: String,
    pub model: String,
    pub api_type: String,
    pub provider: Option<String>,
    pub context_window_tokens: u64,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}

/// Non-sensitive projection of configured routes, not an inference routing authority.
///
/// Inference publishes this resource only after successful route compilation.
/// Agent admission clones an entry into its startup environment; a reload affects
/// subsequent admissions, not the `agent_info` snapshot of existing Lua drivers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelRuntimeCatalog {
    models: HashMap<String, ModelRuntimeInfo>,
}
impl Resource for ModelRuntimeCatalog {}
impl ModelRuntimeCatalog {
    pub fn new(models: impl IntoIterator<Item = ModelRuntimeInfo>) -> Self {
        Self {
            models: models
                .into_iter()
                .map(|model| (model.id.clone(), model))
                .collect(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&ModelRuntimeInfo> {
        self.models.get(id)
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}
