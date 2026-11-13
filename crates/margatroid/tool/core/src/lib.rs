pub mod components;
mod error;
pub mod events;
pub mod types;

pub use components::ToolTasks;
pub use error::{ToolError, ToolErrorKind};
pub use events::{InitToolTasks, InitToolTasksReply, ToolExecuteReply, ToolExecuteRequest};
pub use types::{
    TaskReadResult, ToolConfiguration, ToolExecuteResponse, ToolExecutionLimits, ToolImage,
    ToolTaskHandle, ToolTaskSnapshot,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::io::{self, Write};

pub fn parse_tool_result(value: &Value) -> Result<ToolExecuteResponse, ToolError> {
    let mut response = ToolExecuteResponse {
        call_id: String::new(),
        alias: String::new(),
        content: String::new(),
        images: Vec::new(),
        failed: false,
    };
    if let Some(fields) = value.as_object() {
        if let (Some(ok), Some(content)) = (
            fields.get("ok").and_then(Value::as_bool),
            fields.get("content").and_then(Value::as_str),
        ) {
            response.content = content.to_owned();
            response.failed = !ok;
            if let Some(images) = fields.get("images") {
                response.images = serde_json::from_value(images.clone())
                    .map_err(|_| protocol_error("invalid tool result images"))?;
            }
            validate_images(&response.images)?;
            if !ok && !response.images.is_empty() {
                return Err(protocol_error("failed tool result must not contain images"));
            }
            return Ok(response);
        }
        if fields.contains_key("images") {
            return Err(protocol_error(
                "images require a valid tool result envelope",
            ));
        }
    }
    response.content = match value {
        Value::String(text) => text.clone(),
        _ => {
            serde_json::to_string(value).map_err(|_| protocol_error("cannot encode tool result"))?
        }
    };
    Ok(response)
}

fn validate_images(images: &[ToolImage]) -> Result<(), ToolError> {
    for image in images {
        if !matches!(
            image.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            return Err(protocol_error("unsupported tool image media type"));
        }
        if image.data.is_empty() || !image.data.len().is_multiple_of(4) {
            return Err(protocol_error(
                "tool image requires canonical standard base64",
            ));
        }
        let padding = image
            .data
            .bytes()
            .rev()
            .take_while(|&byte| byte == b'=')
            .count();
        if padding > 2 {
            return Err(protocol_error("invalid tool image base64 padding"));
        }
    }
    for image in images {
        let bytes = STANDARD
            .decode(&image.data)
            .map_err(|_| protocol_error("invalid tool image base64"))?;
        let valid = match image.media_type.as_str() {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
            "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
            _ => false,
        };
        if !valid {
            return Err(protocol_error(
                "tool image signature does not match its media type",
            ));
        }
    }
    Ok(())
}

fn protocol_error(message: &str) -> ToolError {
    ToolError::new(ToolErrorKind::RunnerProtocolFailed, message)
}

fn oversized() -> ToolError {
    ToolError::new(ToolErrorKind::OutputTooLarge, "tool data exceeds limit")
}

pub(crate) fn encoded_len(value: &Value, limit: usize) -> Result<usize, ToolError> {
    let mut writer = BoundedCounter { bytes: 0, limit };
    serde_json::to_writer(&mut writer, value).map_err(|_| oversized())?;
    Ok(writer.bytes)
}

struct BoundedCounter {
    bytes: usize,
    limit: usize,
}

impl Write for BoundedCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(buffer.len())
            .filter(|&size| size <= self.limit)
            .ok_or_else(|| io::Error::other("tool data exceeds limit"))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

mod sync;
pub use sync::{AgentTaskSnapshot, TaskSyncBinding, TaskUpdateContext};
