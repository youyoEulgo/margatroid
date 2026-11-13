use crate::{InferenceHandle, InferenceParameters};
use core_plugin::Component;
#[derive(Default)]
pub struct ActiveInference {
    pub current: Option<InferenceHandle>,
}
impl Component for ActiveInference {}
#[derive(Clone, Debug)]
pub struct AgentInferenceConfig {
    data: AgentInferenceData,
    updates: client_core::UpdateProbe<Option<PublicModelInfo>>,
}
#[derive(Clone, Debug)]
pub struct AgentInferenceData {
    pub parameters: InferenceParameters,
}
impl std::ops::Deref for AgentInferenceConfig {
    type Target = AgentInferenceData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl AgentInferenceConfig {
    pub fn new(
        parameters: InferenceParameters,
        updates: client_core::UpdateProbe<Option<PublicModelInfo>>,
    ) -> Self {
        Self {
            data: AgentInferenceData { parameters },
            updates,
        }
    }
    pub fn set(&mut self, parameters: InferenceParameters, routes: &crate::ModelRoutes) {
        self.data.parameters = parameters;
        self.publish_current(routes);
    }
    pub fn publish_current(&self, routes: &crate::ModelRoutes) {
        self.updates.publish(
            routes
                .get(&self.parameters.model)
                .map(|route| PublicModelInfo {
                    id: self.parameters.model.clone(),
                    model: route.model.clone(),
                    provider: route.provider.clone(),
                    api_type: route.api_type.clone(),
                    context_window_tokens: route.context_window_tokens,
                    thinking: route.thinking.clone(),
                    reasoning_effort: route.reasoning_effort.clone(),
                }),
        );
    }
}
impl Component for AgentInferenceConfig {}
#[derive(Clone, Debug)]
pub struct PublicModelInfo {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub api_type: String,
    pub context_window_tokens: u64,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}
