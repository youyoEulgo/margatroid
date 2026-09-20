use agent_plugin::Agent;
use core_plugin::World;
use margatroid_types::{
    Block, BlockInner, BlockPath, InnerType, Message, RefBlock, ResourceId, ToolCall,
};
use resource_id_plugin::WorldResourceIdExt;

use crate::{
    BlockFieldDeclaration, MclBinding, MclCommandRequest, MclCommandValue, MclDomainValue,
    MclEffectCommand, MclError, MclInjectSource, MclOperation, MclSelector, RefMergeDeclaration,
};

fn normalize_command(command: &str) -> String {
    let mut normalized = String::new();
    let mut in_selector = false;
    let mut pending_space = false;
    for character in command.chars() {
        match character {
            '[' => {
                in_selector = true;
                if pending_space {
                    normalized.push(' ');
                    pending_space = false;
                }
                normalized.push(character);
            }
            ']' => {
                in_selector = false;
                normalized.push(character);
            }
            ',' if in_selector => {
                normalized.push(',');
                pending_space = false;
            }
            character if character.is_whitespace() => {
                pending_space = true;
            }
            character => {
                if pending_space
                    && !normalized.ends_with('[')
                    && (!in_selector || !normalized.ends_with(','))
                {
                    normalized.push(' ');
                }
                pending_space = false;
                normalized.push(character);
            }
        }
    }
    normalized.trim().to_owned()
}

pub fn parse_operation(
    command: &str,
    binding: Option<&serde_json::Value>,
) -> Result<MclOperation, MclError> {
    if command.contains(';') {
        return Err(MclError::InvalidCommand);
    }
    let command = normalize_command(command);
    let words = command.split_whitespace().collect::<Vec<_>>();
    match words.first().copied().unwrap_or("") {
        "IMPORT" if words.len() == 4 && words[2] == "AS" => {
            reject_binding(binding)?;
            validate_identifier(words[3])?;
            Ok(MclOperation::Import {
                resource_id: words[1].parse().map_err(|_| MclError::InvalidResourceId)?,
                alias: words[3].into(),
            })
        }
        "CREATE" => {
            reject_binding(binding)?;
            parse_create(&command)
        }
        "GET" if words.len() == 2 => {
            reject_binding(binding)?;
            Ok(MclOperation::Get {
                selector: parse_selector(words[1])?,
            })
        }
        "MERGE" if words.len() >= 4 && words[words.len() - 2] == "FROM" => {
            reject_binding(binding)?;
            let from = words.len() - 2;
            Ok(MclOperation::Merge {
                sources: words[1..from]
                    .iter()
                    .map(|inner| path(words[from + 1], inner.trim_end_matches(',')))
                    .collect::<Result<_, _>>()?,
            })
        }
        "REF_MERGE" if words.len() >= 4 && words[words.len() - 2] == "FROM" => {
            reject_binding(binding)?;
            let from = words.len() - 2;
            Ok(MclOperation::RefMerge {
                sources: words[1..from]
                    .iter()
                    .map(|inner| path(words[from + 1], inner.trim_end_matches(',')))
                    .collect::<Result<_, _>>()?,
            })
        }
        "INJECT" => parse_inject(&words, binding),
        "BIND" if words.len() == 5 && words[2] == "TO" && words[3] == "STATE" => {
            reject_binding(binding)?;
            validate_identifier(words[1])?;
            validate_identifier(words[4])?;
            Ok(MclOperation::BindState {
                block_id: words[1].to_owned(),
                state_name: words[4].to_owned(),
            })
        }
        "LOAD" if words.len() == 5 && words[1] == "STATE" && words[3] == "INTO" => {
            reject_binding(binding)?;
            validate_identifier(words[2])?;
            validate_identifier(words[4])?;
            Ok(MclOperation::LoadState {
                state_name: words[2].to_owned(),
                block_id: words[4].to_owned(),
            })
        }
        "EXPOSE" if words.len() == 2 => {
            let fields = binding
                .and_then(serde_json::Value::as_object)
                .ok_or(MclError::BindingMissing)?;
            validate_identifier(words[1])?;
            let mappings = fields
                .iter()
                .map(|(field, selector)| {
                    validate_identifier(field)?;
                    let selector = selector.as_str().ok_or(MclError::TypeMismatch)?;
                    parse_selector(selector)?;
                    Ok((field.clone(), selector.to_owned()))
                })
                .collect::<Result<Vec<_>, MclError>>()?;
            Ok(MclOperation::Expose {
                domain: words[1].to_owned(),
                mappings,
            })
        }
        "EMIT" if words.get(1) == Some(&"EFFECT") => parse_effect(&words, binding),
        _ => Err(MclError::InvalidCommand),
    }
}

fn parse_create(command: &str) -> Result<MclOperation, MclError> {
    let open = command.find('(').ok_or(MclError::ParseFailed)?;
    let close = command.rfind(')').ok_or(MclError::ParseFailed)?;
    let head = command[..open].split_whitespace().collect::<Vec<_>>();
    if head.len() != 3 {
        return Err(MclError::InvalidCommand);
    }
    let block_id = head[2].to_owned();
    validate_identifier(&block_id)?;
    let body = command[open + 1..close].trim();
    if head[1] == "BLOCK" {
        let mut fields = Vec::new();
        let words = body.split_whitespace().collect::<Vec<_>>();
        let mut cursor = 0;
        while cursor < words.len() {
            if words[cursor] == "," {
                cursor += 1;
                continue;
            }
            if words[cursor] == "MERGE" {
                let as_index = words[cursor..]
                    .iter()
                    .position(|word| *word == "AS")
                    .map(|index| index + cursor)
                    .ok_or(MclError::InvalidCommand)?;
                if as_index + 1 >= words.len() {
                    return Err(MclError::InvalidCommand);
                }
                let segment = &words[cursor..=as_index + 1];
                cursor = as_index + 2;
                let from = segment
                    .iter()
                    .position(|word| *word == "FROM")
                    .ok_or(MclError::InvalidCommand)?;
                if from < 2 || segment.get(from + 2) != Some(&"AS") {
                    return Err(MclError::InvalidCommand);
                }
                let source_block = segment
                    .get(from + 1)
                    .ok_or(MclError::InvalidCommand)?
                    .to_string();
                validate_identifier(&source_block)?;
                let target = segment
                    .get(from + 3)
                    .ok_or(MclError::InvalidCommand)?
                    .trim_end_matches(',')
                    .to_string();
                validate_identifier(&target)?;
                let sources = segment[1..from]
                    .iter()
                    .map(|inner| path(&source_block, inner.trim_end_matches(',')))
                    .collect::<Result<_, _>>()?;
                fields.push(BlockFieldDeclaration::Merge {
                    inner_id: target,
                    sources,
                });
                continue;
            }
            if cursor + 1 >= words.len() {
                return Err(MclError::InvalidCommand);
            }
            let inner_id = words[cursor];
            let kind = words[cursor + 1].trim_end_matches(',');
            validate_identifier(inner_id)?;
            let inner_type = match kind {
                "MESSAGE" => InnerType::Message,
                "RESOURCE" => InnerType::ResourceId,
                _ => return Err(MclError::TypeMismatch),
            };
            fields.push(BlockFieldDeclaration::Empty {
                inner_id: inner_id.to_owned(),
                inner_type,
            });
            cursor += 2;
        }
        Ok(MclOperation::CreateBlock { block_id, fields })
    } else if head[1] == "REF_BLOCK" {
        let mut merges = Vec::new();
        let mut starts = body
            .match_indices("REF_MERGE")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        starts.push(body.len());
        for window in starts.windows(2) {
            let segment = body[window[0]..window[1]]
                .trim()
                .trim_end_matches(',')
                .trim();
            let words = segment.split_whitespace().collect::<Vec<_>>();
            if words.first() != Some(&"REF_MERGE") {
                return Err(MclError::InvalidCommand);
            }
            let from = words
                .iter()
                .position(|w| *w == "FROM")
                .ok_or(MclError::InvalidCommand)?;
            if words.get(from + 2) != Some(&"AS") || from < 2 {
                return Err(MclError::InvalidCommand);
            }
            let source_block = words
                .get(from + 1)
                .ok_or(MclError::InvalidCommand)?
                .to_string();
            let merge_id = words
                .get(from + 3)
                .ok_or(MclError::InvalidCommand)?
                .to_string();
            validate_identifier(&merge_id)?;
            let sources = words[1..from]
                .iter()
                .map(|inner_id| BlockPath {
                    block_id: source_block.clone(),
                    inner_id: inner_id.trim_end_matches(',').into(),
                })
                .collect();
            merges.push(RefMergeDeclaration { merge_id, sources });
        }
        Ok(MclOperation::CreateRefBlock { block_id, merges })
    } else {
        Err(MclError::InvalidCommand)
    }
}

fn parse_inject(
    words: &[&str],
    binding: Option<&serde_json::Value>,
) -> Result<MclOperation, MclError> {
    let to_index = words
        .iter()
        .position(|word| *word == "TO")
        .ok_or(MclError::InvalidCommand)?;
    if to_index < 2 || to_index + 1 >= words.len() || to_index + 2 != words.len() {
        return Err(MclError::InvalidCommand);
    }
    let source_words = &words[1..to_index];
    let target = parse_selector(words[to_index + 1])?;
    let source = if source_words == ["[]"] {
        reject_binding(binding)?;
        MclInjectSource::Bindings(vec![MclBinding(serde_json::Value::Array(Vec::new()))])
    } else if source_words == ["?"] {
        MclInjectSource::Bindings(vec![MclBinding(
            binding.cloned().ok_or(MclError::BindingMissing)?,
        )])
    } else if source_words.len() == 1 && source_words[0].contains('.') {
        MclInjectSource::Selector(parse_selector(source_words[0])?)
    } else {
        let mut values = Vec::new();
        for raw in source_words {
            let raw = raw.trim_end_matches(',');
            if raw.is_empty() || raw == "?" {
                return Err(MclError::InvalidCommand);
            }
            if binding.is_some() {
                return Err(MclError::InvalidCommand);
            }
            validate_identifier(raw)?;
            values.push(MclBinding(serde_json::Value::String(raw.to_owned())));
        }
        MclInjectSource::Bindings(values)
    };
    Ok(MclOperation::Inject { source, target })
}

fn parse_selector(value: &str) -> Result<MclSelector, MclError> {
    let (path_value, suffix) = match value.find('[') {
        Some(index) => {
            if !value.ends_with(']') {
                return Err(MclError::InvalidCommand);
            }
            (&value[..index], Some(&value[index + 1..value.len() - 1]))
        }
        None => (value, None),
    };
    let (block_id, inner_id) = path_value.split_once('.').ok_or(MclError::InvalidCommand)?;
    let path = path(block_id, inner_id)?;
    match suffix {
        None => Ok(MclSelector::All(path)),
        Some(value) => {
            let values = value.split(',').collect::<Vec<_>>();
            match values.as_slice() {
                [index] => Ok(MclSelector::Index {
                    path,
                    index: index.parse().map_err(|_| MclError::InvalidCommand)?,
                }),
                [start, end] => {
                    let start = start.parse().map_err(|_| MclError::InvalidCommand)?;
                    let end = end.parse().map_err(|_| MclError::InvalidCommand)?;
                    if start >= end || (start < 0) != (end < 0) {
                        return Err(MclError::InvalidCommand);
                    }
                    Ok(MclSelector::Range { path, start, end })
                }
                _ => Err(MclError::InvalidCommand),
            }
        }
    }
}

fn parse_effect(
    words: &[&str],
    binding: Option<&serde_json::Value>,
) -> Result<MclOperation, MclError> {
    let effect = words.get(2).copied().ok_or(MclError::EffectInvalid)?;
    match effect {
        "start" if words.len() == 3 => {
            reject_binding(binding)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::Start,
            })
        }
        "finish" if words.len() == 3 => {
            reject_binding(binding)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::Finish,
            })
        }
        "inference" if words.len() == 5 && words[3] == "FROM" => {
            reject_binding(binding)?;
            validate_identifier(words[4])?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::Inference {
                    ref_block_id: words[4].to_owned(),
                },
            })
        }
        "catch_inference" if words.len() == 5 && words[3] == "FROM" => {
            reject_binding(binding)?;
            validate_identifier(words[4])?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::CatchInference {
                    ref_block_id: words[4].to_owned(),
                },
            })
        }
        "history_append" if words == ["EMIT", "EFFECT", "history_append", "FROM", "?"] => {
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::HistoryAppend {
                    message: parse_message(required_binding(binding)?)?,
                },
            })
        }
        "sandbox_use" if words == ["EMIT", "EFFECT", "sandbox_use", "FROM", "?"] => {
            let value = required_binding(binding)?;
            let aliases = if value.is_array() {
                value
                    .as_array()
                    .ok_or(MclError::TypeMismatch)?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or(MclError::TypeMismatch)
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else if value.is_object() {
                if value.as_object().is_none_or(|fields| !fields.is_empty()) {
                    return Err(MclError::TypeMismatch);
                }
                Vec::new()
            } else {
                vec![value.as_str().ok_or(MclError::TypeMismatch)?.to_owned()]
            };
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::SandboxUse { aliases },
            })
        }
        "history_record" if words == ["EMIT", "EFFECT", "history_record", "FROM", "?"] => {
            let value = required_binding(binding)?;
            let kind = value
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or(MclError::TypeMismatch)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::HistoryRecord {
                    kind: kind.to_owned(),
                    content: serde_json::json!({
                        "command": value.get("cmd").cloned().unwrap_or(serde_json::Value::Null),
                    })
                    .to_string(),
                    payload: serde_json::json!({
                        "args": value.get("arg").cloned().unwrap_or(serde_json::Value::Null),
                    })
                    .to_string(),
                },
            })
        }
        "visibility_source" if words.len() == 5 && words[3] == "FROM" => {
            reject_binding(binding)?;
            let MclSelector::All(source) = parse_selector(words[4])? else {
                return Err(MclError::InvalidCommand);
            };
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::VisibilitySource { source },
            })
        }
        "default_visibility_source" if words.len() == 5 && words[3] == "FROM" => {
            reject_binding(binding)?;
            let MclSelector::All(source) = parse_selector(words[4])? else {
                return Err(MclError::InvalidCommand);
            };
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::DefaultVisibilitySource { source },
            })
        }
        "tool_call" if words == ["EMIT", "EFFECT", "tool_call", "FROM", "?"] => {
            let calls: Vec<ToolCall> = serde_json::from_value(required_binding(binding)?.clone())
                .map_err(|_| MclError::ToolCallInvalid)?;
            if calls.is_empty()
                || calls.iter().any(|call| {
                    call.id.is_empty() || call.tool_name.is_empty() || call.arguments.is_empty()
                })
                || calls
                    .iter()
                    .enumerate()
                    .any(|(index, call)| calls[..index].iter().any(|other| other.id == call.id))
            {
                return Err(MclError::ToolCallInvalid);
            }
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::ToolCall { calls },
            })
        }
        _ => Err(MclError::EffectInvalid),
    }
}

fn selector_to_string(selector: &MclSelector) -> String {
    match selector {
        MclSelector::All(path) => format!("{}.{}", path.block_id, path.inner_id),
        MclSelector::Index { path, index } => {
            format!("{}.{}[{}]", path.block_id, path.inner_id, index)
        }
        MclSelector::Range { path, start, end } => {
            format!("{}.{}[{},{}]", path.block_id, path.inner_id, start, end)
        }
    }
}

fn refresh_exposed(
    resources: &mut agent_plugin::AgentResourceMap,
    mcl: &agent_plugin::AgentMcl,
) -> Result<(), MclError> {
    let mut exposed = std::collections::BTreeMap::new();
    for (domain, mappings) in &resources.expose_mappings {
        let mut values = std::collections::BTreeMap::new();
        for (field, selector) in mappings {
            let selector = parse_selector(selector)?;
            let value = crate::command_value_to_json(get_selector_value(mcl, &selector)?)?;
            values.insert(field.clone(), value);
        }
        exposed.insert(domain.clone(), values);
    }
    resources.exposed = exposed;
    Ok(())
}

fn selector_path(selector: &MclSelector) -> BlockPath {
    match selector {
        MclSelector::All(path)
        | MclSelector::Index { path, .. }
        | MclSelector::Range { path, .. } => path.clone(),
    }
}

fn get_selector_value(
    mcl: &agent_plugin::AgentMcl,
    selector: &MclSelector,
) -> Result<MclDomainValue, MclError> {
    let values = select_selector(mcl, selector)?;
    match selector {
        MclSelector::Index { index: _, .. } => match values {
            BlockInner::Message(mut values) => Ok(values
                .pop()
                .map(MclDomainValue::Message)
                .unwrap_or(MclDomainValue::Unit)),
            BlockInner::ResourceId(mut values) => Ok(values
                .pop()
                .map(|value| MclDomainValue::Inner(BlockInner::ResourceId(vec![value])))
                .unwrap_or(MclDomainValue::Unit)),
        },
        MclSelector::All(_) | MclSelector::Range { .. } => Ok(MclDomainValue::Inner(values)),
    }
}

fn select_selector(
    mcl: &agent_plugin::AgentMcl,
    selector: &MclSelector,
) -> Result<BlockInner, MclError> {
    let path = selector_path(selector);
    let values = mcl.select(&path).map_err(|_| MclError::TypeMismatch)?;
    match selector {
        MclSelector::All(_) => Ok(values),
        MclSelector::Index { index, .. } => {
            let index = resolve_index(*index, values.len())?;
            slice_inner(&values, index, index + 1)
        }
        MclSelector::Range { start, end, .. } => {
            let (start, end) = resolve_range(*start, *end, values.len())?;
            slice_inner(&values, start, end)
        }
    }
}

fn resolve_index(index: i64, length: usize) -> Result<usize, MclError> {
    let index = if index >= 0 {
        index as usize
    } else {
        length
            .checked_sub(index.unsigned_abs() as usize)
            .ok_or(MclError::TypeMismatch)?
    };
    (index < length)
        .then_some(index)
        .ok_or(MclError::TypeMismatch)
}

fn resolve_range(start: i64, end: i64, length: usize) -> Result<(usize, usize), MclError> {
    if start >= 0 {
        let end = end as usize;
        let start = start as usize;
        if end <= length && start < end {
            return Ok((start, end));
        }
    } else {
        let start = start
            .checked_add(length as i64 + 1)
            .map(|value| value as usize);
        let end = end
            .checked_add(length as i64 + 1)
            .map(|value| value as usize);
        if let (Some(start), Some(end)) = (start, end) {
            if start < end && end <= length {
                return Ok((start, end));
            }
        }
    }
    Err(MclError::TypeMismatch)
}

fn slice_inner(values: &BlockInner, start: usize, end: usize) -> Result<BlockInner, MclError> {
    match values {
        BlockInner::Message(values) => Ok(BlockInner::Message(values[start..end].to_vec())),
        BlockInner::ResourceId(values) => Ok(BlockInner::ResourceId(values[start..end].to_vec())),
    }
}

fn append_inner(target: &mut BlockInner, values: BlockInner) -> Result<(), MclError> {
    match (target, values) {
        (BlockInner::Message(target), BlockInner::Message(values)) => target.extend(values),
        (BlockInner::ResourceId(target), BlockInner::ResourceId(values)) => target.extend(values),
        _ => return Err(MclError::TypeMismatch),
    }
    Ok(())
}

fn path(block_id: &str, inner_id: &str) -> Result<BlockPath, MclError> {
    validate_identifier(block_id)?;
    validate_identifier(inner_id)?;
    Ok(BlockPath {
        block_id: block_id.to_owned(),
        inner_id: inner_id.to_owned(),
    })
}

fn validate_identifier(value: &str) -> Result<(), MclError> {
    let mut bytes = value.bytes();
    let valid_first = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    if !valid_first
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        || matches!(value, "." | "..")
    {
        return Err(MclError::InvalidCommand);
    }
    Ok(())
}

fn required_binding<'a>(
    binding: Option<&'a serde_json::Value>,
) -> Result<&'a serde_json::Value, MclError> {
    binding.ok_or(MclError::BindingMissing)
}

fn reject_binding(binding: Option<&serde_json::Value>) -> Result<(), MclError> {
    if binding.is_some() {
        Err(MclError::InvalidCommand)
    } else {
        Ok(())
    }
}

fn parse_message(value: &serde_json::Value) -> Result<margatroid_types::MclMessage, MclError> {
    message_from_lua_json(value.clone())
}

fn message_from_lua_json(
    value: serde_json::Value,
) -> Result<margatroid_types::MclMessage, MclError> {
    let message_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or(MclError::TypeMismatch)?;
    match message_type {
        "system" => Ok(margatroid_types::MclMessage::new(
            Message::System {
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            None,
        )),
        "user" => Ok(margatroid_types::MclMessage::new(
            Message::User {
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            None,
        )),
        "assistant" => Ok(margatroid_types::MclMessage::new(
            Message::Assistant {
                reasoning: value
                    .get("reasoning")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                tool_calls: match value.get("tool_calls").cloned() {
                    Some(serde_json::Value::Array(tool_calls)) => {
                        serde_json::from_value(serde_json::Value::Array(tool_calls))
                            .map_err(|_| MclError::TypeMismatch)?
                    }
                    Some(serde_json::Value::Object(tool_calls)) if tool_calls.is_empty() => {
                        Vec::new()
                    }
                    _ => Vec::new(),
                },
            },
            value
                .get("usage")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| MclError::TypeMismatch)?,
        )),
        "tool" => Ok(margatroid_types::MclMessage::new(
            Message::Tool {
                resource_id: serde_json::from_value(
                    value
                        .get("resource_id")
                        .cloned()
                        .ok_or(MclError::TypeMismatch)?,
                )
                .map_err(|_| MclError::TypeMismatch)?,
                tool_call_id: value
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                failed: value
                    .get("failed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            },
            None,
        )),
        "error" => Ok(margatroid_types::MclMessage::new(
            Message::Error {
                message: value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            None,
        )),
        _ => Err(MclError::TypeMismatch),
    }
}

pub fn execute_direct_operation(
    world: &mut World,
    request: &MclCommandRequest,
    operation: MclOperation,
) -> Result<MclCommandValue, MclError> {
    let entity = world
        .entity_by_resource_id(&request.agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    let dependency_sources = world
        .get_component::<Agent>(entity)
        .map(|agent| agent.info.image_sources.clone())
        .unwrap_or_default();
    let agent = world
        .get_component_mut::<Agent>(entity)
        .ok_or(MclError::AgentMissing)?;
    let mut changed: Option<BlockPath> = None;
    let value = match operation {
        MclOperation::CreateBlock { block_id, fields } => {
            let mut block = Block::default();
            for field in fields {
                match field {
                    BlockFieldDeclaration::Empty {
                        inner_id,
                        inner_type,
                    } => {
                        if block
                            .inners
                            .insert(inner_id, empty_inner(inner_type))
                            .is_some()
                        {
                            return Err(MclError::InvalidCommand);
                        }
                    }
                    BlockFieldDeclaration::Merge { inner_id, sources } => {
                        let merged = agent
                            .mcl
                            .merge(&sources)
                            .map_err(|_| MclError::TypeMismatch)?;
                        if block.inners.insert(inner_id, merged).is_some() {
                            return Err(MclError::InvalidCommand);
                        }
                    }
                }
            }
            agent
                .mcl
                .create_block(block_id, block)
                .map_err(|_| MclError::TypeMismatch)?;
            MclDomainValue::Unit
        }
        MclOperation::CreateRefBlock { block_id, merges } => {
            let mut block = RefBlock::default();
            for merge in merges {
                block.merges.insert(
                    merge.merge_id,
                    agent
                        .mcl
                        .ref_merge(&merge.sources)
                        .map_err(|_| MclError::TypeMismatch)?,
                );
            }
            agent
                .mcl
                .create_ref_block(block_id, block)
                .map_err(|_| MclError::TypeMismatch)?;
            MclDomainValue::Unit
        }
        MclOperation::Get { selector } => get_selector_value(&agent.mcl, &selector)?,
        MclOperation::BindState {
            block_id,
            state_name,
        } => {
            agent
                .mcl
                .bind_state(block_id.clone(), state_name.clone())
                .map_err(|_| MclError::TypeMismatch)?;
            let block = agent
                .mcl
                .block(&block_id)
                .map_err(|_| MclError::BlockMissing {
                    assembly: "agent".into(),
                    block: block_id.clone(),
                })?;
            let value = serde_json::to_string(&block).map_err(|_| MclError::TypeMismatch)?;
            agent
                .memory
                .set_state(&state_name, &value)
                .map_err(|_| MclError::ImportMissing("state could not be written".into()))?;
            MclDomainValue::Unit
        }
        MclOperation::LoadState {
            state_name,
            block_id,
        } => {
            let value = agent
                .memory
                .state_value(&state_name)
                .map_err(|_| MclError::ImportMissing("state could not be read".into()))?;
            if let Some(value) = value {
                let block = serde_json::from_str(&value).map_err(|_| MclError::TypeMismatch)?;
                agent
                    .mcl
                    .merge_block(&block_id, block)
                    .map_err(|_| MclError::TypeMismatch)?;
            }
            MclDomainValue::Unit
        }
        MclOperation::Expose { domain, mappings } => {
            let mut parsed = std::collections::BTreeMap::new();
            for (field, selector) in mappings {
                let selector = parse_selector(&selector)?;
                select_selector(&agent.mcl, &selector)?;
                parsed.insert(field, selector_to_string(&selector));
            }
            agent.resources.expose_mappings.insert(domain, parsed);
            refresh_exposed(&mut agent.resources, &agent.mcl)?;
            MclDomainValue::Unit
        }
        MclOperation::Inject { source, target } => {
            let target_path = selector_path(&target);
            let target_values = agent
                .mcl
                .select(&target_path)
                .map_err(|_| MclError::TypeMismatch)?;
            let source_values = match source {
                MclInjectSource::Selector(selector) => select_selector(&agent.mcl, &selector)?,
                MclInjectSource::Bindings(values) => {
                    let mut result = empty_inner(target_values.inner_type());
                    for value in values {
                        let next = {
                            binding_to_inner(
                                &value.0,
                                target_values.inner_type(),
                                &agent.resources.aliases,
                                &dependency_sources,
                            )?
                        };
                        append_inner(&mut result, next)?;
                    }
                    result
                }
            };
            match target {
                MclSelector::All(_) => agent
                    .mcl
                    .cover(&target_path, source_values)
                    .map_err(|_| MclError::TypeMismatch)?,
                MclSelector::Index { index, .. } => {
                    let actual = resolve_index(index, target_values.len())?;
                    agent
                        .mcl
                        .insert_at(&target_path, actual, index < 0, source_values)
                        .map_err(|_| MclError::TypeMismatch)?;
                }
                MclSelector::Range { start, end, .. } => {
                    let (start, end) = resolve_range(start, end, target_values.len())?;
                    agent
                        .mcl
                        .replace_range(&target_path, start, end, source_values)
                        .map_err(|_| MclError::TypeMismatch)?;
                }
            }
            changed = Some(target_path);
            MclDomainValue::Unit
        }
        _ => return Err(MclError::EffectInvalid),
    };
    if let (Some(changed), Some(source)) =
        (changed.as_ref(), agent.resources.visible_source.clone())
    {
        if changed == &source {
            if let Ok(margatroid_types::BlockInner::ResourceId(values)) = agent.mcl.select(&source)
            {
                agent.resources.visible = values.into_iter().collect();
            }
        }
    }
    if let (Some(changed), Some(source)) = (
        changed.as_ref(),
        agent.resources.default_visible_source.clone(),
    ) {
        if changed == &source {
            if let Ok(margatroid_types::BlockInner::ResourceId(values)) = agent.mcl.select(&source)
            {
                agent.resources.default_visible = values.into_iter().collect();
            }
        }
    }
    if changed.is_some() {
        refresh_exposed(&mut agent.resources, &agent.mcl)?;
    }
    let state_write = changed.as_ref().and_then(|changed| {
        agent
            .mcl
            .state_bindings()
            .get(&changed.block_id)
            .cloned()
            .map(|state_name| (state_name, changed.block_id.clone()))
    });
    let state_value = if let Some((state_name, block_id)) = state_write {
        let block = agent
            .mcl
            .block(&block_id)
            .map_err(|_| MclError::BlockMissing {
                assembly: "agent".into(),
                block: block_id,
            })?;
        let value = serde_json::to_string(&block).map_err(|_| MclError::TypeMismatch)?;
        Some((state_name, value))
    } else {
        None
    };
    let memory = agent.memory.clone();
    if let Some((state_name, value)) = state_value {
        memory
            .set_state(&state_name, &value)
            .map_err(|_| MclError::ImportMissing("state could not be written".into()))?;
    }
    Ok(value)
}

pub fn history_record(
    world: &mut World,
    agent_id: &ResourceId,
    kind: String,
    content: String,
    payload: String,
    source: &str,
) -> Result<MclDomainValue, MclError> {
    let entity = world
        .entity_by_resource_id(agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    world.emit_event(margatroid_types::AgentHistoryRecordWriteRequested {
        agent: entity,
        source: source.to_owned(),
        kind,
        content,
        payload,
    });
    Ok(MclDomainValue::Unit)
}

fn empty_inner(kind: InnerType) -> BlockInner {
    match kind {
        InnerType::Message => BlockInner::Message(Vec::new()),
        InnerType::ResourceId => BlockInner::ResourceId(Vec::new()),
    }
}
fn binding_to_inner(
    value: &serde_json::Value,
    kind: InnerType,
    aliases: &std::collections::HashMap<String, ResourceId>,
    sources: &std::collections::HashMap<ResourceId, std::sync::Arc<str>>,
) -> Result<BlockInner, MclError> {
    match kind {
        InnerType::Message => {
            if let Some(alias) = value.as_str() {
                let resource = aliases.get(alias).ok_or_else(|| {
                    MclError::ImportMissing(format!("alias `{alias}` is not imported"))
                })?;
                let content = sources.get(resource).ok_or_else(|| {
                    MclError::ImportMissing(format!("content for `{resource}` is unavailable"))
                })?;
                let message = if resource.scope() == "system" {
                    Message::System {
                        content: content.to_string(),
                    }
                } else {
                    Message::User {
                        content: content.to_string(),
                    }
                };
                return Ok(BlockInner::Message(vec![
                    margatroid_types::MclMessage::new(message, None),
                ]));
            }
            if value.is_array() {
                let values = value
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(message_from_lua_json)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(BlockInner::Message(values))
            } else {
                parse_message(value).map(|v| BlockInner::Message(vec![v]))
            }
        }
        InnerType::ResourceId => {
            if let Some(alias) = value.as_str() {
                if let Some(resource) = aliases.get(alias) {
                    return Ok(BlockInner::ResourceId(vec![resource.clone()]));
                }
                let resource = alias.parse::<ResourceId>().map_err(|_| {
                    MclError::ImportMissing(format!("invalid resource id `{alias}`"))
                })?;
                return Ok(BlockInner::ResourceId(vec![resource]));
            }
            if value.is_array() {
                serde_json::from_value(value.clone())
                    .map(BlockInner::ResourceId)
                    .map_err(|_| MclError::TypeMismatch)
            } else {
                value
                    .as_str()
                    .ok_or(MclError::TypeMismatch)
                    .and_then(|v| v.parse().map_err(|_| MclError::TypeMismatch))
                    .map(|v| BlockInner::ResourceId(vec![v]))
            }
        }
    }
}

pub fn history_append(
    world: &mut World,
    agent_id: &ResourceId,
    message: margatroid_types::MclMessage,
    fallback_turn_id: &str,
    source: &str,
) -> Result<MclDomainValue, MclError> {
    let entity = world
        .entity_by_resource_id(agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    let agent = world
        .get_component_mut::<Agent>(entity)
        .ok_or(MclError::AgentMissing)?;
    let turn_id = agent
        .turn
        .turn_id
        .clone()
        .unwrap_or_else(|| fallback_turn_id.to_owned());
    let tool_schema = if matches!(message.message, Message::Assistant { .. }) {
        agent
            .inference
            .pending
            .get(&(entity, turn_id.clone()))
            .map(|pending| pending.tool_schema.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    world.emit_event(margatroid_types::AgentHistoryMessageWriteRequested {
        id: turn_id,
        source: source.to_owned(),
        agent: entity,
        message: message.message,
        tool_schema,
        usage: message.usage,
    });
    Ok(MclDomainValue::Unit)
}
pub fn domain_to_command(value: MclDomainValue) -> MclCommandValue {
    value
}
