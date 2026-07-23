use super::types::*;
use types::ChatRequest;

pub fn to_wire(req: &ChatRequest) -> WireRequest {
    WireRequest {
        model: req.model.clone(),
        messages: req
            .messages
            .iter()
            .filter_map(|m| serde_json::to_value(m).ok())
            .collect(),
        stream: req.stream,
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        tools: req.tools.as_ref().map(|t| {
            t.iter()
                .filter_map(|v| serde_json::to_value(v).ok())
                .collect()
        }),
        tool_choice: req
            .tool_choice
            .as_ref()
            .and_then(|c| serde_json::to_value(c).ok()),
        stream_options: req.stream.and_then(|s| {
            if s {
                Some(StreamOptions {
                    include_usage: true,
                })
            } else {
                None
            }
        }),
    }
}
