use crate::{InferenceHandle, InferenceParameters};
use core_plugin::Component;
#[derive(Default)]
pub struct ActiveInference {
    pub current: Option<InferenceHandle>,
}
impl Component for ActiveInference {}
#[derive(Clone, Debug)]
pub struct AgentInferenceConfig {
    pub parameters: InferenceParameters,
}
impl Component for AgentInferenceConfig {}
