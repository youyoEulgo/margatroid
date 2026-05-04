//! 通用类型 ↔ OpenRouter wire format 双向转换
use super::types::*;
use types::{ChatRequest, ChatResponse, StreamChunk, Usage};

/// ChatRequest → WireRequest
pub fn to_wire(req: &ChatRequest) -> WireRequest {
    // provider_options 里读取 OpenRouter 专属参数
    let provider_prefs = req
        .provider_options
        .get("openrouter_provider")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    WireRequest {
        model: Some(req.model.clone()),
        messages: Some(
            req.messages
                .iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect(),
        ),
        stream: req.stream,
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        frequency_penalty: req.frequency_penalty,
        presence_penalty: req.presence_penalty,
        seed: req.seed,
        stop: req.stop.as_ref().map(|s| serde_json::to_value(s).unwrap()),
        tools: req.tools.as_ref().map(|t| serde_json::to_value(t).unwrap()),
        tool_choice: req
            .tool_choice
            .as_ref()
            .map(|t| serde_json::to_value(t).unwrap()),
        response_format: req
            .response_format
            .as_ref()
            .map(|r| serde_json::to_value(r).unwrap()),
        logit_bias: req
            .logit_bias
            .as_ref()
            .map(|l| serde_json::to_value(l).unwrap()),
        user: req.user.clone(),
        provider: provider_prefs,
        ..Default::default()
    }
}

/// WireResponse → ChatResponse
pub fn from_wire(wire: WireResponse) -> ChatResponse {
    ChatResponse {
        id: wire.id,
        model: wire.model,
        created: wire.created,
        choices: wire
            .choices
            .into_iter()
            .enumerate()
            .filter_map(|(i, v)| from_wire_choice(i as u32, v))
            .collect(),
        usage: wire.usage.map(from_wire_usage),
    }
}

/// WireResponse（streaming chunk）→ StreamChunk
pub fn from_wire_stream(wire: WireResponse) -> StreamChunk {
    StreamChunk {
        id: wire.id,
        model: wire.model,
        choices: wire
            .choices
            .into_iter()
            .enumerate()
            .filter_map(|(i, v)| from_wire_stream_choice(i as u32, v))
            .collect(),
        usage: wire.usage.map(from_wire_usage),
    }
}

fn from_wire_choice(index: u32, v: serde_json::Value) -> Option<types::ResponseChoice> {
    let choice: WireNonStreamingChoice = serde_json::from_value(v).ok()?;
    let message = serde_json::from_value(choice.message).ok()?;
    Some(types::ResponseChoice {
        index,
        message,
        finish_reason: choice
            .finish_reason
            .as_deref()
            .and_then(parse_finish_reason),
    })
}

fn from_wire_stream_choice(index: u32, v: serde_json::Value) -> Option<types::StreamChoice> {
    let choice: WireStreamingChoice = serde_json::from_value(v).ok()?;
    let delta = serde_json::from_value(choice.delta).ok()?;
    Some(types::StreamChoice {
        index,
        delta,
        finish_reason: choice
            .finish_reason
            .as_deref()
            .and_then(parse_finish_reason),
    })
}

fn from_wire_usage(wire: WireUsage) -> Usage {
    Usage {
        prompt_tokens: wire.prompt_tokens,
        completion_tokens: wire.completion_tokens,
        total_tokens: wire.total_tokens,
        cost: wire.cost,
        prompt_tokens_details: None,
        completion_tokens_details: None,
    }
}

fn parse_finish_reason(s: &str) -> Option<types::FinishReason> {
    match s {
        "stop" => Some(types::FinishReason::Stop),
        "length" => Some(types::FinishReason::Length),
        "tool_calls" => Some(types::FinishReason::ToolCalls),
        "content_filter" => Some(types::FinishReason::ContentFilter),
        _ => Some(types::FinishReason::Other),
    }
}
