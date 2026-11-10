mod error;
pub mod types;
pub use error::{ProtocolError, ProtocolErrorDto};
pub use types::*;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;

pub const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ID_BYTES: usize = 512;
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;

fn invalid(message: &str) -> ProtocolError {
    ProtocolError::new("InvalidRequest", message)
}

pub fn valid_request_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= MAX_ID_BYTES && !id.chars().any(char::is_control)
}

pub fn parse_client_message(text: &str) -> Result<ClientMessage, ProtocolError> {
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::new(
            "MessageTooLarge",
            "message exceeds 32 MiB",
        ));
    }
    let raw: Value = serde_json::from_str(text).map_err(|_| invalid("invalid protocol message"))?;
    if matches!(
        raw.get("type").and_then(Value::as_str),
        Some("mcl.command" | "history.query" | "agent.turn.abort")
    ) && raw
        .get("message")
        .and_then(|message| message.get("agent"))
        .is_none_or(|agent| !agent.is_string())
    {
        return Err(invalid(
            "an explicit agent address is required in message.agent",
        ));
    }
    if !raw.get("message").is_some_and(Value::is_object) {
        return Err(invalid("message must be an explicit object"));
    }
    let message: ClientMessage = serde_json::from_value(raw)
        .map_err(|_| invalid("invalid protocol message shape or unknown field"))?;
    if !valid_request_id(message.id()) {
        return Err(invalid(
            "request id must be nonempty, contain no control characters and fit 512 bytes",
        ));
    }
    match &message {
        ClientMessage::ConnectionRegister { message, .. } => {
            if message.client_type.is_empty()
                || message.client_type.len() > 128
                || !message.client_type.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
            {
                return Err(invalid("invalid client_type"));
            }
            if message
                .name
                .as_deref()
                .is_some_and(|name| !logical_name(name, 512))
            {
                return Err(invalid("invalid client name"));
            }
        }
        ClientMessage::WorkspaceStart { message, .. }
        | ClientMessage::WorkspaceRestart { message, .. } => validate_path(&message.path)?,
        ClientMessage::WorkspaceStop { message, .. } => validate_workspace(&message.workspace)?,
        ClientMessage::StateQuery { .. } | ClientMessage::WorkspaceDefinitionsQuery { .. } => (),
        ClientMessage::HistoryQuery { message, .. } => {
            validate_workspace(&message.workspace)?;
            validate_address(&message.agent, "agent")?;
        }
        ClientMessage::MclCommand { message, .. } => {
            validate_workspace(&message.workspace)?;
            validate_address(&message.agent, "agent")?;
            if message.command.trim().is_empty()
                || message.command.len() > MAX_COMMAND_BYTES
                || message.command.contains('\0')
                || message.arguments.len() > 128
            {
                return Err(invalid("invalid command text or argument count"));
            }
            let mut nodes = 0;
            for argument in &message.arguments {
                validate_value(
                    argument,
                    0,
                    &mut nodes,
                    message.command.split_whitespace().next() != Some("CONFIG"),
                )?;
            }
        }
        ClientMessage::AgentTurnAbort { message, .. } => {
            validate_workspace(&message.workspace)?;
            validate_address(&message.agent, "agent")?;
            if !valid_request_id(&message.agent_instance) || !valid_request_id(&message.turn_id) {
                return Err(invalid(
                    "explicit agent_instance and turn_id must be valid identifiers",
                ));
            }
        }
        ClientMessage::AgentStart { message, .. } | ClientMessage::AgentRestart { message, .. } => {
            validate_address(&message.workspace, "workspace")?;
            if !logical_name(&message.name, 256) {
                return Err(invalid("invalid Agent member name"));
            }
            validate_address(&message.parameters.driver, "driver")?;
            validate_path(&message.parameters.home_root)?;
        }
    }
    Ok(message)
}

pub fn encode_server_message(message: &ServerMessage) -> Result<String, ProtocolError> {
    let text = serde_json::to_string(message)
        .map_err(|_| ProtocolError::new("EncodeFailed", "response cannot be serialized"))?;
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::new(
            "ResponseTooLarge",
            "response exceeds 32 MiB",
        ));
    }
    Ok(text)
}

fn logical_name(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum
        && !matches!(value, "." | "..")
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
}

fn validate_path(path: &str) -> Result<(), ProtocolError> {
    if path.trim().is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        Err(invalid("invalid filesystem path"))
    } else {
        Ok(())
    }
}

fn validate_workspace(workspace: &WorkspaceReferenceDto) -> Result<(), ProtocolError> {
    validate_address(workspace.address(), "workspace")?;
    if let WorkspaceReferenceDto::Reference {
        name, project_root, ..
    } = workspace
        && (name.len() > 512
            || project_root.len() > 4096
            || name
                .chars()
                .chain(project_root.chars())
                .any(char::is_control))
    {
        return Err(invalid("invalid workspace reference"));
    }
    Ok(())
}

fn validate_address(value: &str, expected: &str) -> Result<(), ProtocolError> {
    if value.len() > 4096 {
        return Err(invalid("address exceeds size limit"));
    }
    let (kind, rest) = value
        .split_once(':')
        .ok_or_else(|| invalid("invalid address format"))?;
    if kind != expected {
        return Err(invalid("address has the wrong kind"));
    }
    let (scope, remainder) = rest
        .split_once('/')
        .ok_or_else(|| invalid("invalid address format"))?;
    let (name, tag) = remainder
        .split_once(':')
        .map_or((remainder, None), |(name, tag)| (name, Some(tag)));
    let valid = |part: &str| {
        !part.is_empty()
            && !matches!(part, "." | "..")
            && !part
                .chars()
                .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
    };
    if !valid(scope) || !valid(name) || tag.is_some_and(|tag| !valid(tag)) {
        return Err(invalid("invalid address segment"));
    }
    Ok(())
}

fn validate_value(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
    inspect_images: bool,
) -> Result<(), ProtocolError> {
    *nodes += 1;
    if depth > 64 || *nodes > 100_000 {
        return Err(invalid("argument structure exceeds limits"));
    }
    match value {
        Value::Array(values) => {
            for value in values {
                validate_value(value, depth + 1, nodes, inspect_images)?;
            }
        }
        Value::Object(values) => {
            if inspect_images && values.contains_key("media_type") && values.contains_key("data") {
                validate_image(value)?;
            }
            if inspect_images && let Some(Value::Array(images)) = values.get("data") {
                if images.len() > 8 {
                    return Err(invalid("message has too many images"));
                }
                let mut decoded_total = 0usize;
                for image in images {
                    validate_image(image)?;
                    let data = image["data"].as_str().unwrap();
                    let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count();
                    decoded_total += data.len() / 4 * 3 - padding;
                }
                if decoded_total > 20 * 1024 * 1024 {
                    return Err(invalid("message images exceed total size limit"));
                }
            }
            for value in values.values() {
                validate_value(value, depth + 1, nodes, inspect_images)?;
            }
        }
        _ => (),
    }
    Ok(())
}

fn validate_image(value: &Value) -> Result<(), ProtocolError> {
    let media = value
        .get("media_type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("invalid image media type"))?;
    let data = value
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("invalid image data"))?;
    if data.is_empty() || data.len() > (5_usize * 1024 * 1024).div_ceil(3) * 4 {
        return Err(invalid("image exceeds size limit or is empty"));
    }
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| invalid("image must use canonical standard base64"))?;
    let valid = match media {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    };
    if !valid || bytes.len() > 5 * 1024 * 1024 {
        return Err(invalid("image signature or size is invalid"));
    }
    Ok(())
}
