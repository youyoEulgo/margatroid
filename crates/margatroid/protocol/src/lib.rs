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
                || !message.client_type.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-')
                })
            {
                return Err(invalid("invalid client_type"));
            }
            if message
                .name
                .as_deref()
                .is_some_and(|v| !logical_name(v, 512))
            {
                return Err(invalid("invalid client name"));
            }
        }
        ClientMessage::WorkspaceReq { .. }
        | ClientMessage::WorkspaceReload { .. }
        | ClientMessage::AgentReq { .. } => {}
        ClientMessage::WorkspaceStart { message, .. }
        | ClientMessage::WorkspaceStop { message, .. }
        | ClientMessage::WorkspaceRestart { message, .. } => validate_path(&message.path)?,
        ClientMessage::AgentStart { message, .. } => {
            validate_address(&message.workspace, "workspace")?;
            if !logical_name(&message.name, 256) {
                return Err(invalid("invalid Agent member name"));
            }
        }
        ClientMessage::AgentStop { message, .. }
        | ClientMessage::AgentRestart { message, .. }
        | ClientMessage::AgentTurnReq { message, .. }
        | ClientMessage::AgentTaskReq { message, .. }
        | ClientMessage::AgentUiReq { message, .. } => validate_target(&message.address)?,
        ClientMessage::AgentTurnAbort { message, .. } => {
            validate_target(&message.address)?;
            if !valid_request_id(&message.turn) {
                return Err(invalid("invalid turn"));
            }
        }
        ClientMessage::AgentHistoryReq { message, .. }
        | ClientMessage::AgentLogReq { message, .. }
        | ClientMessage::AgentMclReq { message, .. } => {
            validate_target(&message.address)?;
            if message.before_sequence.is_some_and(|v| v > i64::MAX as u64)
                || message.after_sequence.is_some_and(|v| v > i64::MAX as u64)
                || message.limit == Some(0)
                || matches!((message.after_sequence,message.before_sequence),(Some(a),Some(b)) if a>=b)
            {
                return Err(invalid("invalid record range"));
            }
        }
        ClientMessage::MclExecute { message, .. } => {
            validate_target(&message.address)?;
            if message.command.trim().is_empty()
                || message.command.len() > MAX_COMMAND_BYTES
                || message.command.contains('\0')
                || message.arguments.len() > 128
            {
                return Err(invalid("invalid command text or argument count"));
            }
            let mut nodes = 0;
            for value in &message.arguments {
                validate_value(
                    value,
                    0,
                    &mut nodes,
                    message.command.split_whitespace().next() != Some("CONFIG"),
                )?;
            }
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

fn validate_target(address: &str) -> Result<(), ProtocolError> {
    validate_address(address, "agent")
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
            if inspect_images
                && values.get("type").and_then(Value::as_str).is_some_and(|t| {
                    matches!(
                        t,
                        "system" | "user" | "assistant" | "tool_call" | "inject" | "error"
                    )
                })
            {
                if values.contains_key("replay")
                    || values.contains_key("images")
                    || values.contains_key("api_format")
                {
                    return Err(invalid("removed message field"));
                }
                if values
                    .get("sequence")
                    .is_some_and(|v| v.as_u64().is_none_or(|n| n == 0 || n > i64::MAX as u64))
                {
                    return Err(invalid("invalid message sequence"));
                }
            }
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

pub(crate) fn validate_server_value(value: &Value) -> Result<(), ProtocolError> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("missing response type"))?;
    let sync = matches!(
        kind,
        "workspace.sync"
            | "agent.sync"
            | "agent.ui.sync"
            | "agent.turn.sync"
            | "agent.task.sync"
            | "agent.log.sync"
            | "agent.mcl.sync"
            | "agent.message.chunk.sync"
            | "agent.message.full.sync"
    );
    let known = sync
        || matches!(
            kind,
            "connection.register.result"
                | "workspace.res"
                | "workspace.start.result"
                | "workspace.stop.result"
                | "workspace.restart.result"
                | "workspace.reload.result"
                | "agent.res"
                | "agent.ui.res"
                | "agent.start.result"
                | "agent.stop.result"
                | "agent.restart.result"
                | "agent.turn.res"
                | "agent.turn.abort.result"
                | "agent.task.res"
                | "agent.history.res"
                | "agent.log.res"
                | "agent.mcl.res"
                | "mcl.execute.result"
        );
    if !known {
        return Err(invalid("unknown response type"));
    }
    if sync {
        if value.get("id").is_some() {
            return Err(invalid("sync cannot carry request id"));
        }
    } else {
        if !value
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(valid_request_id)
        {
            return Err(invalid("invalid response id"));
        }
        let success = value
            .get("success")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("response success must be boolean"))?;
        let error = value.get("error");
        if success && error.is_some() || !success && error.is_none_or(|v| !v.is_object()) {
            return Err(invalid("success and error are mutually exclusive"));
        }
        let payload = match kind {
            "connection.register.result" => Some("client"),
            "workspace.res" => Some("workspaces"),
            "agent.res" => Some("agents"),
            "agent.ui.res" => Some("ui"),
            "agent.turn.res" => Some("state"),
            "agent.task.res" => Some("tasks"),
            "agent.history.res" | "agent.log.res" | "agent.mcl.res" => Some("records"),
            _ => None,
        };
        if let Some(key) = payload
            && (success && (value.get(key).is_none() || key != "ui" && value[key].is_null())
                || !success && value.get(key).is_some())
        {
            return Err(invalid("response payload does not match success"));
        }
        if !success && value.get("data").is_some() {
            return Err(invalid("failed command cannot contain data"));
        }
        if matches!(
            kind,
            "agent.history.res" | "agent.log.res" | "agent.mcl.res"
        ) {
            for key in ["next_before_sequence", "next_after_sequence"] {
                if success {
                    let cursor = value
                        .get(key)
                        .ok_or_else(|| invalid("successful page requires both cursors"))?;
                    if !cursor.is_null()
                        && !cursor
                            .as_u64()
                            .is_some_and(|n| n > 0 && n <= i64::MAX as u64)
                    {
                        return Err(invalid("invalid page cursor"));
                    }
                } else if value.get(key).is_some() {
                    return Err(invalid("failed page cannot contain cursors"));
                }
            }
        }
    }
    if kind == "agent.ui.sync" {
        require_fields(value, &["ui", "error"])?;
        if !value["error"].is_null() && !value["ui"].is_null() {
            return Err(invalid("failed UI sync must have null ui"));
        }
    }
    if matches!(kind, "agent.ui.res" | "agent.ui.sync")
        && let Some(ui) = value.get("ui").filter(|v| !v.is_null())
    {
        let bundle: AgentUiBundleDto =
            serde_json::from_value(ui.clone()).map_err(|_| invalid("invalid Driver UI bundle"))?;
        validate_ui_bundle(&bundle)?;
    }
    if let Some(error) = value.get("error").filter(|v| !v.is_null()) {
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("missing error code"))?;
        if code.chars().any(char::is_control)
            || error
                .get("message")
                .and_then(Value::as_str)
                .is_none_or(|s| s.len() > 512)
        {
            return Err(invalid("invalid response error"));
        }
    }
    if value.get("success") != Some(&Value::Bool(false))
        && let Some(address) = value.get("address").and_then(Value::as_str)
    {
        validate_address(address, "agent")?;
    }
    for (key, expected) in [("workspaces", "workspace"), ("agents", "agent")] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            for item in items {
                require_fields(
                    item,
                    if expected == "workspace" {
                        &["address", "error"]
                    } else {
                        &["sandboxes", "sandbox", "model", "error"]
                    },
                )?;
                if expected == "workspace" {
                    validate_path(
                        item["path"]
                            .as_str()
                            .ok_or_else(|| invalid("missing workspace path"))?,
                    )?;
                    if let Some(address) = item["address"].as_str() {
                        validate_address(address, "workspace")?;
                    }
                    if !matches!(
                        item["status"].as_str(),
                        Some(
                            "inactive"
                                | "starting"
                                | "running"
                                | "restarting"
                                | "stopping"
                                | "failed"
                        )
                    ) {
                        return Err(invalid("invalid workspace status"));
                    }
                } else {
                    validate_target(
                        item["address"]
                            .as_str()
                            .ok_or_else(|| invalid("missing agent address"))?,
                    )?;
                    validate_address(
                        item["workspace"]
                            .as_str()
                            .ok_or_else(|| invalid("missing workspace address"))?,
                        "workspace",
                    )?;
                    if !matches!(
                        item["status"].as_str(),
                        Some(
                            "starting" | "ready" | "working" | "restarting" | "stopping" | "failed"
                        )
                    ) {
                        return Err(invalid("invalid active agent status"));
                    }
                    if let Some(model) = item.get("model").filter(|value| !value.is_null()) {
                        require_fields(model, &["thinking", "reasoning_effort"])?;
                    }
                    if let Some(tools) = item.get("tools").and_then(Value::as_array) {
                        for tool in tools {
                            require_fields(tool, &["owner_task_id", "binding"])?;
                            if !tool["input_schema"].is_object() {
                                return Err(invalid("tool input_schema must be an object"));
                            }
                        }
                    }
                }
            }
        }
    }
    if kind == "connection.register.result" && value["success"] == true {
        validate_address(
            value["client"]["address"]
                .as_str()
                .ok_or_else(|| invalid("missing client address"))?,
            "client",
        )?;
    }
    if kind == "agent.turn.sync" {
        require_fields(value, &["turn"])?;
    }
    if kind == "agent.turn.res" && value["success"] == true {
        require_fields(&value["state"], &["turn"])?;
    }
    if matches!(kind, "agent.task.res" | "agent.task.sync")
        && let Some(tasks) = value.get("tasks").and_then(Value::as_array)
    {
        for task in tasks {
            require_fields(task, &["error"])?;
        }
    }
    if kind == "agent.message.full.sync" {
        require_fields(value, &["turn", "generation", "history_sequence", "error"])?;
    }
    if kind == "agent.message.chunk.sync" {
        if !value
            .get("generation")
            .and_then(Value::as_str)
            .is_some_and(valid_request_id)
            || !value
                .get("turn")
                .and_then(Value::as_str)
                .is_some_and(valid_request_id)
            || !value
                .get("sequence")
                .and_then(Value::as_u64)
                .is_some_and(|n| n > 0)
        {
            return Err(invalid("invalid generation chunk"));
        }
        let delta = value
            .get("delta")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("missing delta"))?;
        if delta.is_empty()
            || delta
                .iter()
                .any(|(k, v)| !matches!(k.as_str(), "content" | "reasoning") || !v.is_string())
        {
            return Err(invalid("invalid delta"));
        }
    }
    if kind == "agent.message.full.sync" {
        let message = value
            .get("message")
            .ok_or_else(|| invalid("missing full message"))?;
        if value.get("history_sequence").and_then(Value::as_u64)
            != message.get("sequence").and_then(Value::as_u64)
        {
            return Err(invalid("full history sequence mismatch"));
        }
    }
    if let Some(records) = value.get("records").and_then(Value::as_array) {
        let mut previous = 0;
        for record in records {
            let sequence = record
                .get("sequence")
                .and_then(Value::as_u64)
                .filter(|v| *v > previous && *v <= i64::MAX as u64)
                .ok_or_else(|| invalid("records must have ascending positive sequences"))?;
            previous = sequence;
            if matches!(kind, "agent.log.res" | "agent.log.sync") {
                require_fields(
                    record,
                    &[
                        "details_json",
                        "context_json",
                        "origin",
                        "raw_error",
                        "traceback",
                    ],
                )?;
                if !matches!(
                    record["level"].as_str(),
                    Some("debug" | "info" | "warn" | "error")
                ) || !matches!(record["source"].as_str(), Some("driver" | "tool" | "host"))
                {
                    return Err(invalid("invalid agent log source or level"));
                }
            }
            if kind == "agent.history.res"
                && (record.get("kind").and_then(Value::as_str) != Some("message")
                    || record
                        .get("payload")
                        .and_then(|p| p.get("sequence"))
                        .and_then(Value::as_u64)
                        != Some(sequence))
            {
                return Err(invalid("history sequence or kind mismatch"));
            }
        }
        if matches!(kind, "agent.log.sync" | "agent.mcl.sync") && records.is_empty() {
            return Err(invalid("record sync requires new entries"));
        }
    }
    let mut nodes = 0;
    validate_value(value, 0, &mut nodes, false)?;
    validate_internal_message_fields(value)?;
    Ok(())
}
fn validate_internal_message_fields(value: &Value) -> Result<(), ProtocolError> {
    match value {
        Value::Array(items) => {
            for item in items {
                validate_internal_message_fields(item)?;
            }
        }
        Value::Object(fields) => {
            if fields.get("type").and_then(Value::as_str).is_some_and(|t| {
                matches!(
                    t,
                    "system" | "user" | "assistant" | "tool_call" | "inject" | "error"
                )
            }) {
                if fields.contains_key("replay")
                    || fields.contains_key("api_format")
                    || fields.contains_key("images")
                {
                    return Err(invalid("removed internal message field"));
                }
                if fields
                    .get("sequence")
                    .is_some_and(|v| !v.as_u64().is_some_and(|n| n > 0 && n <= i64::MAX as u64))
                {
                    return Err(invalid("invalid persisted sequence"));
                }
            }
            for item in fields.values() {
                validate_internal_message_fields(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn require_fields(value: &Value, keys: &[&str]) -> Result<(), ProtocolError> {
    for key in keys {
        if value.get(*key).is_none() {
            return Err(invalid("required nullable field is missing"));
        }
    }
    Ok(())
}

pub fn validate_ui_bundle(bundle: &AgentUiBundleDto) -> Result<(), ProtocolError> {
    let Some(hash) = bundle.revision.strip_prefix("sha256:") else {
        return Err(invalid("invalid UI revision"));
    };
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("invalid UI revision"));
    }
    if bundle.entry != "index.html" || !bundle.files.iter().any(|file| file.path == bundle.entry) {
        return Err(invalid("missing UI entry"));
    }
    let mut previous: Option<&str> = None;
    for file in &bundle.files {
        if file.path.starts_with('/')
            || file.path.contains('\\')
            || file.path.chars().any(char::is_control)
            || file
                .path
                .split('/')
                .any(|p| p.is_empty() || matches!(p, "." | ".."))
            || previous.is_some_and(|old| old >= file.path.as_str())
        {
            return Err(invalid("UI paths must be unique sorted relative paths"));
        }
        if file.media_type.trim().is_empty()
            || file.media_type.chars().any(char::is_control)
            || STANDARD.decode(&file.data).is_err()
        {
            return Err(invalid("invalid UI file encoding or media type"));
        }
        previous = Some(&file.path);
    }
    Ok(())
}
