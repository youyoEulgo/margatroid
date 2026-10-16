use crate::message_types::{AgentMessage, ToolDefinition};

/// Owned inference input independent of transport, API format, and execution ownership.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DomainRequest {
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<ToolDefinition>,
    pub config: DomainRequestConfig,
}

/// Common model request semantics; an empty default model is not an executable route.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DomainRequestConfig {
    pub model: String,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub stop: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_request_has_no_invented_model_or_parameters() {
        let request = DomainRequest::default();
        assert!(request.messages.is_empty());
        assert!(request.tools.is_empty());
        assert!(request.config.model.is_empty());
        assert_eq!(request.config.temperature, None);
        assert_eq!(request.config.max_output_tokens, None);
        assert_eq!(request.config.top_p, None);
        assert!(request.config.stop.is_empty());
    }
}
