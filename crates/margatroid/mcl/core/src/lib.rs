mod components;
mod error;
mod events;
mod types;

pub use components::MclQueue;
use core_plugin::{Entity, World};
pub use error::{MclError, MclErrorKind};
pub use events::{MclCommand, MclReply};
use std::collections::BTreeSet;
pub use types::*;

pub trait WorldMclExt {
    fn mcl_close(&mut self, target: Entity) -> Result<(), MclError>;
    fn mcl_drain(&self, target: Entity) -> Result<MclDrain, MclError>;
}
impl WorldMclExt for World {
    fn mcl_close(&mut self, target: Entity) -> Result<(), MclError> {
        let queue = self
            .get_component_mut::<MclQueue>(target)
            .ok_or_else(|| MclError::new(MclErrorKind::TargetMissing, "MCL queue missing"))?;
        queue.accepting = false;
        queue.cancel.cancel();
        Ok(())
    }
    fn mcl_drain(&self, target: Entity) -> Result<MclDrain, MclError> {
        self.get_component::<MclQueue>(target)
            .map(|queue| queue.drain.clone())
            .ok_or_else(|| MclError::new(MclErrorKind::TargetMissing, "MCL queue missing"))
    }
}

fn invalid(message: &str) -> MclError {
    MclError::new(MclErrorKind::ParseFailed, message)
}
pub fn validate_identifier(value: &str) -> Result<(), MclError> {
    let mut chars = value.bytes();
    if !chars
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !chars.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(invalid("invalid identifier"));
    }
    Ok(())
}

pub fn parse_selector(text: &str) -> Result<Selector, MclError> {
    let text = text.trim();
    let (path, range) = match text.find('[') {
        Some(index) => {
            let bounds = text[index + 1..]
                .strip_suffix(']')
                .ok_or_else(|| invalid("unclosed selector"))?;
            let range = if let Some((start, end)) = bounds.split_once(',') {
                let start = start
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| invalid("invalid range"))?;
                let end = end
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| invalid("invalid range"))?;
                if start >= end || (start < 0) != (end < 0) {
                    return Err(invalid("range endpoints must be ordered and share sign"));
                }
                Selection::Range(start, end)
            } else {
                Selection::Index(
                    bounds
                        .trim()
                        .parse()
                        .map_err(|_| invalid("invalid index"))?,
                )
            };
            (&text[..index], range)
        }
        None => (text, Selection::All),
    };
    let (block, field) = path
        .split_once('.')
        .ok_or_else(|| invalid("selector requires block.field"))?;
    validate_identifier(block)?;
    validate_identifier(field)?;
    Ok(Selector {
        block: block.into(),
        field: field.into(),
        range,
    })
}

fn expression(text: &str) -> Result<MessageExpression, MclError> {
    let mut depth = 0i32;
    let mut start = 0;
    let mut parts = Vec::new();
    for (index, ch) in text.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(invalid("unbalanced selector"));
                }
            }
            ',' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(invalid("unbalanced selector"));
    }
    parts.push(&text[start..]);
    let sources = parts
        .into_iter()
        .map(|part| {
            let part = part.trim();
            if part.contains('.') {
                Ok(MessageSource::Selector(parse_selector(part)?))
            } else {
                validate_identifier(part)?;
                Ok(MessageSource::PromptAlias(part.into()))
            }
        })
        .collect::<Result<Vec<_>, MclError>>()?;
    Ok(MessageExpression { sources })
}

pub fn parse_instruction(input: &MclInstruction) -> Result<MclOperation, MclError> {
    let text = input.text.trim();
    if text.is_empty() || text.contains(';') {
        return Err(invalid("exactly one instruction is required"));
    }
    let (verb, body) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let body = body.trim();
    let mut count = 0;
    let operation = match verb {
        "CREATE" => {
            let (temporary, body) = if let Some(body) = body.strip_prefix("TEMP ") {
                (true, body.trim())
            } else {
                (false, body)
            };
            let body = body
                .strip_prefix("BLOCK ")
                .ok_or_else(|| invalid("expected BLOCK"))?;
            let (name, fields) = body
                .split_once('(')
                .ok_or_else(|| invalid("expected fields"))?;
            let name = name.trim();
            validate_identifier(name)?;
            let fields = fields
                .strip_suffix(')')
                .ok_or_else(|| invalid("unclosed fields"))?;
            let fields = fields
                .split(',')
                .map(|f| {
                    let f = f.trim();
                    validate_identifier(f)?;
                    Ok(f.to_string())
                })
                .collect::<Result<Vec<_>, MclError>>()?;
            if fields.iter().collect::<BTreeSet<_>>().len() != fields.len() {
                return Err(invalid("duplicate fields"));
            }
            MclOperation::Create {
                name: name.into(),
                fields,
                temporary,
            }
        }
        "IMPORT" => {
            let words = body.split_whitespace().collect::<Vec<_>>();
            let [kind, locator, "AS", alias] = words.as_slice() else {
                return Err(invalid("expected IMPORT kind locator AS alias"));
            };
            let kind = match *kind {
                "MESSAGE" => ImportKind::Message,
                "TOOL" => ImportKind::Tool,
                "SANDBOX" => ImportKind::Sandbox,
                _ => return Err(invalid("invalid import kind")),
            };
            validate_identifier(alias)?;
            MclOperation::Import {
                kind,
                locator: (*locator).into(),
                alias: (*alias).into(),
            }
        }
        "GET" => MclOperation::Get(expression(body)?),
        "INJECT" => {
            let (source, target) = body
                .split_once(" TO ")
                .ok_or_else(|| invalid("expected TO"))?;
            let source = match source.trim() {
                "?" => {
                    count = 1;
                    MessageParameter::Manual(0)
                }
                "[]" => MessageParameter::Empty,
                source => MessageParameter::Expression(expression(source)?),
            };
            MclOperation::Inject {
                source,
                target: parse_selector(target)?,
            }
        }
        "CONFIG" => {
            let words = body.split_whitespace().collect::<Vec<_>>();
            let operation = match words.as_slice() {
                ["GET"] => ConfigOperation::Get,
                ["SET", "?", "?"] => {
                    count = 2;
                    let path = config_path(input.arguments.first())?;
                    let value = input
                        .arguments
                        .get(1)
                        .ok_or_else(|| invalid("missing config value"))?;
                    let value = toml::Value::try_from(value).map_err(|_| {
                        MclError::new(
                            MclErrorKind::ConfigTypeMismatch,
                            "value is not representable as TOML",
                        )
                    })?;
                    ConfigOperation::Set { path, value }
                }
                ["DELETE", "?"] => {
                    count = 1;
                    ConfigOperation::Delete {
                        path: config_path(input.arguments.first())?,
                    }
                }
                _ => return Err(invalid("expected CONFIG GET, SET ? ? or DELETE ?")),
            };
            MclOperation::Config(operation)
        }
        "EFFECT" => {
            let (name, mut rest) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
            let arity = match name {
                "inference" | "tool_call" => 2,
                "sandbox_use" | "history_append" => 1,
                "start_turn" | "finish_turn" | "ready" | "wake" => 0,
                _ => return Err(invalid("unknown effect")),
            };
            let mut arguments = Vec::new();
            rest = rest.trim();
            while !rest.is_empty() {
                if let Some(tail) = rest.strip_prefix('?') {
                    arguments.push(EffectParameter::Manual(count));
                    count += 1;
                    rest = tail.trim();
                } else if let Some(tail) = rest.strip_prefix('(') {
                    let (inner, tail) = tail
                        .split_once(')')
                        .ok_or_else(|| invalid("unclosed effect parameter"))?;
                    arguments.push(EffectParameter::Expression(expression(inner)?));
                    rest = tail.trim();
                } else {
                    return Err(invalid("effect parameter must be expression or ?"));
                }
            }
            if arguments.len() != arity {
                return Err(invalid("effect parameter count mismatch"));
            }
            MclOperation::Effect {
                name: name.into(),
                arguments,
            }
        }
        _ => return Err(invalid("unknown instruction")),
    };
    if input.arguments.len() != count {
        return Err(MclError::new(
            MclErrorKind::ArgumentMismatch,
            "question mark argument count mismatch",
        ));
    }
    Ok(operation)
}

fn config_path(value: Option<&serde_json::Value>) -> Result<String, MclError> {
    let path = value.and_then(|v| v.as_str()).ok_or_else(|| {
        MclError::new(
            MclErrorKind::ConfigPathInvalid,
            "config path must be a string",
        )
    })?;
    if path.split('.').any(|part| {
        part.is_empty()
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, '[' | ']'))
    }) {
        return Err(MclError::new(
            MclErrorKind::ConfigPathInvalid,
            "invalid dotted config path",
        ));
    }
    Ok(path.into())
}
