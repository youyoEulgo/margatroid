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
        ClientMessage::ResourceReq { message, .. } => {
            if message
                .resource_type
                .as_deref()
                .is_some_and(|v| !resource_type(v))
            {
                return Err(invalid("invalid resource type"));
            }
            for part in [&message.scope, &message.name].into_iter().flatten() {
                validate_resource_part(part)?;
            }
        }
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
        ClientMessage::ModelConfigReq { .. }
        | ClientMessage::WorkspaceReq { .. }
        | ClientMessage::AgentReq { .. } => {}
        ClientMessage::WorkspaceCreate { message, .. } => {
            validate_address(&message.address, "workspace")?;
            if message.address == "/" || message.address[1..].contains('/') {
                return Err(invalid("Workspace must be a root child node"));
            }
            if !valid_request_id(&message.alias) {
                return Err(invalid("invalid Workspace alias"));
            }
            validate_absolute_path(&message.project_root)?;
        }
        ClientMessage::WorkspaceDelete { message, .. } => {
            validate_address(&message.address, "workspace")?
        }
        ClientMessage::WorkspaceRename { message, .. } => {
            validate_address(&message.address, "workspace")?;
            if !valid_request_id(&message.alias) {
                return Err(invalid("invalid Workspace alias"));
            }
        }
        ClientMessage::AgentRename { message, .. } => {
            validate_address(&message.workspace, "workspace")?;
            if !valid_request_id(&message.agent_id) {
                return Err(invalid("invalid Agent record id"));
            }
            if message
                .alias
                .as_deref()
                .is_some_and(|alias| !valid_request_id(alias))
            {
                return Err(invalid("invalid Agent alias"));
            }
        }
        ClientMessage::AgentStart { message, .. } => match message {
            AgentStartInputDto::Existing {
                workspace,
                node_name,
            } => {
                validate_address(workspace, "workspace")?;
                validate_node_name(node_name)?;
            }
            AgentStartInputDto::New {
                workspace,
                tool_path,
                arguments,
            } => {
                validate_address(workspace, "workspace")?;
                validate_absolute_path(tool_path)?;
                if !arguments.is_object() {
                    return Err(invalid("Driver arguments must be an object"));
                }
                if let Some(name) = arguments.get("node_name").filter(|v| !v.is_null()) {
                    validate_node_name(
                        name.as_str()
                            .ok_or_else(|| invalid("node_name must be a string or null"))?,
                    )?;
                }
                if let Some(alias) = arguments.get("alias").filter(|v| !v.is_null())
                    && !alias.as_str().is_some_and(valid_request_id)
                {
                    return Err(invalid("invalid Agent alias"));
                }
                validate_value(arguments, 0, &mut 0, false)?;
            }
        },
        ClientMessage::AgentStop { message, .. } => validate_target(&message.address)?,
        ClientMessage::AgentArchive { message, .. } => {
            validate_address(&message.workspace, "workspace")?;
            validate_node_name(&message.node_name)?;
        }
        ClientMessage::AgentTurnReq { message, .. }
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
        ClientMessage::ModelConfigUpdate { message, .. } => {
            if message.content.len() > 1024 * 1024 {
                return Err(invalid("model configuration exceeds budget"));
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

fn resource_type(value: &str) -> bool {
    matches!(value, "driver" | "tool" | "prompt" | "sandbox")
}
fn validate_resource_part(value: &str) -> Result<(), ProtocolError> {
    if value.is_empty()
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return Err(invalid("invalid resource path segment"));
    }
    Ok(())
}

fn validate_resource_path(value: &str) -> Result<(), ProtocolError> {
    validate_absolute_path(value)?;
    if std::path::Path::new(value).components().any(|v| {
        matches!(
            v,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(invalid("resource path contains traversal"));
    }
    Ok(())
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

fn validate_address(value: &str, _expected: &str) -> Result<(), ProtocolError> {
    if value.len() > 4096
        || !value.starts_with('/')
        || value.contains(['\\', ':'])
        || value.chars().any(char::is_control)
        || (value != "/"
            && value[1..]
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | "..")))
    {
        return Err(invalid("invalid absolute node path"));
    }
    Ok(())
}

fn validate_node_name(value: &str) -> Result<(), ProtocolError> {
    if !logical_name(value, 512) || value.starts_with("~temporary-") {
        return Err(invalid("invalid ordinary Agent node name"));
    }
    Ok(())
}

fn validate_absolute_path(value: &str) -> Result<(), ProtocolError> {
    validate_path(value)?;
    if !std::path::Path::new(value).is_absolute() {
        return Err(invalid("filesystem path must be absolute"));
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
                        "system"
                            | "user"
                            | "assistant"
                            | "compact"
                            | "tool_call"
                            | "inject"
                            | "error"
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
        "model.config.sync"
            | "workspace.sync"
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
            "model.config.res"
                | "model.config.update.result"
                | "connection.register.result"
                | "resource.res"
                | "workspace.res"
                | "agent.start.result"
                | "agent.stop.result"
                | "agent.archive.result"
                | "agent.rename.result"
                | "workspace.create.result"
                | "workspace.update.result"
                | "workspace.rename.result"
                | "workspace.delete.result"
                | "agent.res"
                | "agent.ui.res"
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
            "model.config.res" => Some("content"),
            "resource.res" => Some("resources"),
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
    if kind == "resource.res" && value["success"] == true {
        let records = value["resources"]
            .as_array()
            .ok_or_else(|| invalid("resources must be an array"))?;
        let mut previous: Option<(&str, &str, &str, &str)> = None;
        for row in records {
            require_fields(
                row,
                &[
                    "address",
                    "type",
                    "scope",
                    "name",
                    "tag",
                    "path",
                    "tool_path",
                ],
            )?;
            let text = |key: &str| {
                row[key]
                    .as_str()
                    .ok_or_else(|| invalid("resource field must be string"))
            };
            let kind = text("type")?;
            if !resource_type(kind) {
                return Err(invalid("invalid resource type"));
            }
            for key in ["scope", "name", "tag"] {
                validate_resource_part(text(key)?)?;
            }
            let expected = format!(
                "{}:{}/{}:{}",
                kind,
                text("scope")?,
                text("name")?,
                text("tag")?
            );
            if text("address")? != expected {
                return Err(invalid("resource address and fields differ"));
            }
            let key = (kind, text("scope")?, text("name")?, text("tag")?);
            if previous.is_some_and(|previous| previous >= key) {
                return Err(invalid(
                    "resource list must be uniquely sorted by type/scope/name/tag",
                ));
            }
            previous = Some(key);
            let path = text("path")?;
            validate_resource_path(path)?;
            match (kind, row["tool_path"].as_str(), row["tool_path"].is_null()) {
                ("tool", Some(tool), _) if tool == path => {}
                ("driver", Some(tool), _) => {
                    validate_resource_path(tool)?;
                }
                ("driver" | "prompt" | "sandbox", None, true) => {}
                _ => return Err(invalid("invalid resource tool_path")),
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
                if expected == "workspace" {
                    require_fields(
                        item,
                        &["address", "alias", "project_root", "status", "agents"],
                    )?;
                    validate_address(
                        item["address"]
                            .as_str()
                            .ok_or_else(|| invalid("missing workspace address"))?,
                        "workspace",
                    )?;
                    if !item["alias"].as_str().is_some_and(valid_request_id) {
                        return Err(invalid("invalid Workspace alias"));
                    }
                    validate_absolute_path(
                        item["project_root"]
                            .as_str()
                            .ok_or_else(|| invalid("missing project_root"))?,
                    )?;
                    if !matches!(
                        item["status"].as_str(),
                        Some("running" | "deleting" | "error")
                    ) {
                        return Err(invalid("invalid workspace status"));
                    }
                    let records = item["agents"]
                        .as_array()
                        .ok_or_else(|| invalid("missing Agent registrations"))?;
                    let mut ids = std::collections::BTreeSet::new();
                    let mut names = std::collections::BTreeSet::new();
                    for record in records {
                        require_fields(
                            record,
                            &[
                                "id",
                                "parent_node",
                                "node_name",
                                "alias",
                                "category",
                                "tool_path",
                                "tool_arguments",
                                "previous_node_name",
                                "previous_alias",
                            ],
                        )?;
                        let id = record["id"]
                            .as_str()
                            .filter(|v| valid_request_id(v))
                            .ok_or_else(|| invalid("invalid registered Agent id"))?;
                        if !ids.insert(id) {
                            return Err(invalid("duplicate Agent registration id"));
                        }
                        validate_address(
                            record["parent_node"]
                                .as_str()
                                .ok_or_else(|| invalid("missing parent node"))?,
                            "agent",
                        )?;
                        validate_absolute_path(
                            record["tool_path"]
                                .as_str()
                                .ok_or_else(|| invalid("missing Driver path"))?,
                        )?;
                        if !record["tool_arguments"].is_object() {
                            return Err(invalid("Driver arguments must be an object"));
                        }
                        match record["category"].as_str() {
                            Some("normal") => {
                                let name = record["node_name"]
                                    .as_str()
                                    .ok_or_else(|| invalid("normal Agent requires node_name"))?;
                                validate_node_name(name)?;
                                if !names.insert(name) {
                                    return Err(invalid("duplicate Agent node name"));
                                }
                            }
                            Some("temporary" | "archived") if record["node_name"].is_null() => {}
                            _ => {
                                return Err(invalid(
                                    "invalid Agent registration category or node_name",
                                ));
                            }
                        }
                        for key in ["alias", "previous_alias"] {
                            if !record[key].is_null()
                                && !record[key].as_str().is_some_and(valid_request_id)
                            {
                                return Err(invalid("invalid registered Agent alias"));
                            }
                        }
                        if let Some(name) = record["previous_node_name"].as_str() {
                            validate_node_name(name)?;
                        }
                    }
                } else {
                    require_fields(item, &["creator", "sandboxes", "sandbox", "model", "error"])?;
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
                        Some("starting" | "running" | "closing")
                    ) {
                        return Err(invalid("invalid active agent status"));
                    }
                    let creator = item["creator"]
                        .as_object()
                        .ok_or_else(|| invalid("Agent creator must be an object"))?;
                    validate_address(
                        creator
                            .get("address")
                            .and_then(Value::as_str)
                            .ok_or_else(|| invalid("creator address missing"))?,
                        "agent",
                    )?;
                    if !creator
                        .get("task_id")
                        .and_then(Value::as_str)
                        .is_some_and(valid_request_id)
                    {
                        return Err(invalid("invalid creator task id"));
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
    if kind == "model.config.sync" {
        require_fields(value, &["content", "error"])?;
        if value["content"].is_string() && !value["error"].is_null() {
            return Err(invalid(
                "model configuration content and read error are mutually exclusive",
            ));
        }
    }
    if kind == "mcl.execute.result"
        && value["success"] == true
        && value["data"]["kind"] == "turn"
        && !value["data"]["value"]
            .as_str()
            .is_some_and(valid_request_id)
    {
        return Err(invalid("invalid returned turn"));
    }
    if kind == "agent.message.chunk.sync" {
        if !value
            .get("generation")
            .and_then(Value::as_str)
            .is_some_and(valid_request_id)
            || value
                .get("turn")
                .is_none_or(|turn| !turn.is_null() && !turn.as_str().is_some_and(valid_request_id))
            || !value
                .get("sequence")
                .and_then(Value::as_u64)
                .is_some_and(|n| n > 0)
        {
            return Err(invalid("invalid generation chunk"));
        }
        let content = value
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("missing cumulative content"))?;
        let reasoning = value
            .get("reasoning")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("missing cumulative reasoning"))?;
        if content.is_empty() && reasoning.is_empty() {
            return Err(invalid("empty cumulative text snapshot"));
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
    serde_json::from_value::<types::ServerWire>(value.clone())
        .map_err(|_| invalid("invalid response shape or unknown field"))?;
    let mut nodes = 0;
    validate_value(value, 0, &mut nodes, false)?;
    if !kind.starts_with("agent.log.") {
        validate_internal_message_fields(value)?;
    }
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
                    "system" | "user" | "assistant" | "compact" | "tool_call" | "inject" | "error"
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
            for (key, item) in fields {
                if !matches!(key.as_str(), "tool_arguments" | "input_schema" | "binding") {
                    validate_internal_message_fields(item)?;
                }
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
