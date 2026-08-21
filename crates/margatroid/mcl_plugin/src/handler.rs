use agent_plugin::Agent;
use core_plugin::World;
use margatroid_types::{
    Block, BlockInner, BlockPath, InnerType, Message, RefBlock, ResourceId, ToolCall,
};
use resource_id_plugin::WorldResourceIdExt;

use crate::{
    BlockFieldDeclaration, MclBinding, MclCommandRequest, MclCommandValue, MclDomainValue,
    MclEffectCommand, MclError, MclOperation, RefMergeDeclaration,
};

pub fn parse_operation(
    command: &str,
    binding: Option<&serde_json::Value>,
) -> Result<MclOperation, MclError> {
    if command.contains(';') {
        return Err(MclError::InvalidCommand);
    }
    let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
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
        "SELECT" if words.len() == 4 && words[2] == "FROM" => {
            reject_binding(binding)?;
            Ok(MclOperation::Select {
                source: path(words[3], words[1])?,
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
        "DELETE" => parse_delete(&words, binding),
        "INJECT" => parse_inject(&words, binding),
        "EMIT" if words.get(1) == Some(&"EFFECT") => parse_effect(&words, binding),
        _ => Err(MclError::InvalidCommand),
    }
}

use crate::MclPredicate;
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
                "TOOL_CALL" => InnerType::ToolCall,
                "TOOL" => InnerType::ResourceId,
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
    if words.len() == 9
        && words[1] == "SELECT"
        && words[3] == "FROM"
        && words[5] == "COVER"
        && words[7] == "FROM"
    {
        reject_binding(binding)?;
        return Ok(MclOperation::CoverInner {
            source: path(words[4], words[2])?,
            target: path(words[8], words[6])?,
        });
    }
    let to_index = words
        .iter()
        .position(|word| *word == "TO")
        .ok_or(MclError::InvalidCommand)?;
    let from_index = words
        .iter()
        .position(|word| *word == "FROM")
        .ok_or(MclError::InvalidCommand)?;
    if from_index < 2 || from_index + 1 >= words.len() || to_index < 1 || to_index + 1 >= from_index
    {
        return Err(MclError::InvalidCommand);
    }
    let source = words[from_index + 1];
    let target = path(source, words[to_index + 1])?;
    let raw_values = words[1..to_index].to_vec();
    if raw_values.is_empty() {
        return Err(MclError::InvalidCommand);
    }
    let values = raw_values
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let raw = raw.trim_end_matches(',');
            if raw == "?" {
                if raw_values.len() != 1 {
                    return Err(MclError::InvalidCommand);
                }
                binding
                    .cloned()
                    .map(MclBinding)
                    .ok_or(MclError::BindingMissing)
            } else {
                if index != 0 && binding.is_some() {
                    return Err(MclError::InvalidCommand);
                }
                validate_identifier(raw)?;
                if binding.is_some() {
                    return Err(MclError::InvalidCommand);
                }
                Ok(MclBinding(serde_json::Value::String(raw.to_owned())))
            }
        })
        .collect::<Result<Vec<_>, MclError>>()?;
    if values.len() == 1 {
        Ok(MclOperation::Inject {
            target,
            value: values.into_iter().next().expect("length checked"),
        })
    } else {
        Ok(MclOperation::InjectMany { target, values })
    }
}

fn parse_effect(
    words: &[&str],
    binding: Option<&serde_json::Value>,
) -> Result<MclOperation, MclError> {
    match (words.get(2).copied().unwrap_or(""), words.len()) {
        ("start", 3) => {
            reject_binding(binding)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::Start,
            })
        }
        ("finish", 3) => {
            reject_binding(binding)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::Finish,
            })
        }
        ("realtime_load", 3) => {
            reject_binding(binding)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::RealtimeLoad,
            })
        }
        ("realtime_source", 4) => {
            reject_binding(binding)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::RealtimeSource {
                    ref_block_id: effect_ref_block(words[3])?,
                },
            })
        }
        ("inference", 4) => {
            reject_binding(binding)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::Inference {
                    ref_block_id: effect_ref_block(words[3])?,
                },
            })
        }
        ("catch_inference", 4) => {
            reject_binding(binding)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::CatchInference {
                    ref_block_id: effect_ref_block(words[3])?,
                },
            })
        }
        ("history_append", 3 | 4) if words.get(3).is_none_or(|value| *value == "?") => {
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::HistoryAppend {
                    message: parse_message(required_binding(binding)?)?,
                },
            })
        }
        ("visibility_source", 7)
            if words.get(3) == Some(&"(SELECT")
                && words.get(5) == Some(&"FROM")
                && words.get(6).is_some_and(|value| value.ends_with(')')) =>
        {
            reject_binding(binding)?;
            let block_id = words[6].strip_suffix(')').ok_or(MclError::InvalidCommand)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::VisibilitySource {
                    source: path(block_id, words[4])?,
                },
            })
        }
        ("default_visibility_source", 7)
            if words.get(3) == Some(&"(SELECT")
                && words.get(5) == Some(&"FROM")
                && words.get(6).is_some_and(|value| value.ends_with(')')) =>
        {
            reject_binding(binding)?;
            let block_id = words[6].strip_suffix(')').ok_or(MclError::InvalidCommand)?;
            Ok(MclOperation::Emit {
                effect: MclEffectCommand::DefaultVisibilitySource {
                    source: path(block_id, words[4])?,
                },
            })
        }
        ("tool_call", 4) if words[3] == "?" => {
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

fn parse_delete(
    words: &[&str],
    binding: Option<&serde_json::Value>,
) -> Result<MclOperation, MclError> {
    match words {
        ["DELETE", inner, "FROM", block] => {
            reject_binding(binding)?;
            Ok(MclOperation::DeleteAll {
                target: path(block, inner)?,
            })
        }
        ["DELETE", inner, "FIRST", "FROM", block] => {
            reject_binding(binding)?;
            Ok(MclOperation::DeleteFirst {
                target: path(block, inner)?,
            })
        }
        ["DELETE", inner, "FROM", block, "WHERE", "id", "==", "?"] => {
            let value = required_binding(binding)?
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or(MclError::TypeMismatch)?;
            Ok(MclOperation::DeleteWhere {
                target: path(block, inner)?,
                predicate: MclPredicate::IdEquals(value.to_owned()),
            })
        }
        _ => Err(MclError::InvalidCommand),
    }
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

fn effect_ref_block(value: &str) -> Result<String, MclError> {
    let value = value
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .ok_or(MclError::InvalidCommand)?;
    validate_identifier(value)?;
    Ok(value.to_owned())
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
    let mut dependency_sources = world
        .get_component::<Agent>(entity)
        .map(|agent| agent.info.image_sources.clone())
        .unwrap_or_default();
    for (prompt_id, file_name) in [
        ("prompt:system/soul:latest", "SOUL.md"),
        ("prompt:user/compact:latest", "COMPACT.md"),
    ] {
        if let Ok(prompt_id) = prompt_id.parse::<ResourceId>() {
            if !dependency_sources.contains_key(&prompt_id) {
                let prompt_path = world
                    .get_component::<Agent>(entity)
                    .map(|agent| agent.info.image_root.join(file_name))
                    .unwrap_or_default();
                if let Ok(content) = std::fs::read_to_string(&prompt_path) {
                    if !content.trim().is_empty() {
                        dependency_sources.insert(prompt_id, std::sync::Arc::<str>::from(content));
                    }
                }
            }
        }
    }
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
        MclOperation::Select { source } => {
            MclDomainValue::Inner(agent.mcl.select(&source).map_err(|_| {
                MclError::BlockMissing {
                    assembly: "agent".into(),
                    block: source.block_id,
                }
            })?)
        }
        MclOperation::Merge { sources } => MclDomainValue::Inner(
            agent
                .mcl
                .merge(&sources)
                .map_err(|_| MclError::TypeMismatch)?,
        ),
        MclOperation::RefMerge { sources } => MclDomainValue::Paths(
            agent
                .mcl
                .ref_merge(&sources)
                .map_err(|_| MclError::TypeMismatch)?
                .paths()
                .to_vec(),
        ),
        MclOperation::Inject { target, value } => {
            agent
                .mcl
                .insert(
                    &target,
                    binding_to_inner(
                        &value.0,
                        agent
                            .mcl
                            .select(&target)
                            .map_err(|_| MclError::InnerMissing {
                                block: target.block_id.clone(),
                                inner: target.inner_id.clone(),
                            })?
                            .inner_type(),
                        &agent.resources.aliases,
                        &dependency_sources,
                    )?,
                )
                .map_err(|_| MclError::TypeMismatch)?;
            changed = Some(target);
            MclDomainValue::Unit
        }
        MclOperation::InjectMany { target, values } => {
            let kind = agent
                .mcl
                .select(&target)
                .map_err(|_| MclError::InnerMissing {
                    block: target.block_id.clone(),
                    inner: target.inner_id.clone(),
                })?
                .inner_type();
            for value in values {
                agent
                    .mcl
                    .insert(
                        &target,
                        binding_to_inner(
                            &value.0,
                            kind,
                            &agent.resources.aliases,
                            &dependency_sources,
                        )?,
                    )
                    .map_err(|_| MclError::TypeMismatch)?;
            }
            changed = Some(target);
            MclDomainValue::Unit
        }
        MclOperation::CoverValue { target, value } => {
            let kind = agent
                .mcl
                .select(&target)
                .map_err(|_| MclError::TypeMismatch)?
                .inner_type();
            agent
                .mcl
                .cover(
                    &target,
                    binding_to_inner(
                        &value.0,
                        kind,
                        &agent.resources.aliases,
                        &dependency_sources,
                    )?,
                )
                .map_err(|_| MclError::TypeMismatch)?;
            changed = Some(target);
            MclDomainValue::Unit
        }
        MclOperation::CoverInner { source, target } => {
            let values = agent
                .mcl
                .select(&source)
                .map_err(|_| MclError::TypeMismatch)?;
            agent
                .mcl
                .cover(&target, values)
                .map_err(|_| MclError::TypeMismatch)?;
            changed = Some(target);
            MclDomainValue::Unit
        }
        MclOperation::DeleteAll { target } => {
            agent
                .mcl
                .delete(&target, margatroid_types::MclDeleteSelection::All)
                .map_err(|_| MclError::TypeMismatch)?;
            changed = Some(target);
            MclDomainValue::Unit
        }
        MclOperation::DeleteFirst { target } => {
            agent
                .mcl
                .delete(&target, margatroid_types::MclDeleteSelection::First)
                .map_err(|_| MclError::TypeMismatch)?;
            changed = Some(target);
            MclDomainValue::Unit
        }
        MclOperation::DeleteWhere {
            target,
            predicate: MclPredicate::IdEquals(id),
        } => {
            let values = agent
                .mcl
                .select(&target)
                .map_err(|_| MclError::InnerMissing {
                    block: target.block_id.clone(),
                    inner: target.inner_id.clone(),
                })?;
            let indices = match values {
                BlockInner::ToolCall(calls) => calls
                    .iter()
                    .enumerate()
                    .filter_map(|(index, call)| (call.id == id).then_some(index))
                    .collect::<Vec<_>>(),
                BlockInner::ResourceId(resources) => resources
                    .iter()
                    .enumerate()
                    .filter_map(|(index, resource)| (resource.to_string() == id).then_some(index))
                    .collect::<Vec<_>>(),
                _ => return Err(MclError::TypeMismatch),
            };
            agent
                .mcl
                .delete(
                    &target,
                    margatroid_types::MclDeleteSelection::Indices(indices),
                )
                .map_err(|_| MclError::TypeMismatch)?;
            changed = Some(target);
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
    if let (Some(changed), Some(source)) =
        (changed.as_ref(), agent.mcl().realtime_source().cloned())
    {
        if !source
            .dependencies
            .iter()
            .any(|dependency| dependency == changed)
        {
            return Ok(value);
        }
        let values = agent
            .mcl
            .select(&BlockPath {
                block_id: source.ref_block_id,
                inner_id: source.message_merge_id,
            })
            .map_err(|_| MclError::MessageSourceUnavailable)?;
        let BlockInner::Message(messages) = values else {
            return Err(MclError::TypeMismatch);
        };
        world.emit_event(margatroid_types::AgentRealtimeContextWriteRequested {
            agent: entity,
            messages: messages.clone(),
        });
    }
    Ok(value)
}

pub fn realtime_source(
    world: &mut World,
    agent_id: &ResourceId,
    ref_block_id: String,
) -> Result<MclDomainValue, MclError> {
    let entity = world
        .entity_by_resource_id(agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    let snapshot =
        {
            let agent = world
                .get_component_mut::<Agent>(entity)
                .ok_or(MclError::AgentMissing)?;
            let block = agent.mcl.ref_blocks().blocks.get(&ref_block_id).ok_or(
                MclError::RefBlockMissing {
                    assembly: "agent".into(),
                    block: ref_block_id.clone(),
                },
            )?;
            let mut messages = block
                .merges
                .iter()
                .filter(|(_, merge)| matches!(merge, margatroid_types::RefMerge::Message(_)));
            let Some((merge_id, margatroid_types::RefMerge::Message(paths))) = messages.next()
            else {
                return Err(MclError::MessageSourceUnavailable);
            };
            if messages.next().is_some() {
                return Err(MclError::TypeMismatch);
            }
            let source = margatroid_types::MclRealtimeSource {
                ref_block_id: ref_block_id.clone(),
                message_merge_id: merge_id.clone(),
                dependencies: paths.clone(),
            };
            let values = agent
                .mcl
                .select(&BlockPath {
                    block_id: ref_block_id,
                    inner_id: merge_id.clone(),
                })
                .map_err(|_| MclError::MessageSourceUnavailable)?;
            let BlockInner::Message(snapshot) = values else {
                return Err(MclError::TypeMismatch);
            };
            agent.mcl_mut().set_realtime_source(source.clone());
            snapshot
        };
    world.emit_event(margatroid_types::AgentRealtimeContextWriteRequested {
        agent: entity,
        messages: snapshot,
    });
    Ok(MclDomainValue::Unit)
}

fn empty_inner(kind: InnerType) -> BlockInner {
    match kind {
        InnerType::Message => BlockInner::Message(Vec::new()),
        InnerType::ToolCall => BlockInner::ToolCall(Vec::new()),
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
                let resource = aliases.get(alias).ok_or(MclError::ImportMissing)?;
                let content = sources.get(resource).ok_or(MclError::ImportMissing)?;
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
        InnerType::ToolCall => {
            if let Some(alias) = value.as_str() {
                let resource = aliases.get(alias).ok_or(MclError::ImportMissing)?;
                return Ok(BlockInner::ResourceId(vec![resource.clone()]));
            }
            if value.is_array() {
                serde_json::from_value(value.clone())
                    .map(BlockInner::ToolCall)
                    .map_err(|_| MclError::TypeMismatch)
            } else {
                serde_json::from_value::<ToolCall>(value.clone())
                    .map(|v| BlockInner::ToolCall(vec![v]))
                    .map_err(|_| MclError::TypeMismatch)
            }
        }
        InnerType::ResourceId => {
            if let Some(alias) = value.as_str() {
                if let Some(resource) = aliases.get(alias) {
                    return Ok(BlockInner::ResourceId(vec![resource.clone()]));
                }
                let resource = alias
                    .parse::<ResourceId>()
                    .map_err(|_| MclError::ImportMissing)?;
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
        agent: entity,
        message: message.message,
        tool_schema,
        usage: message.usage,
    });
    Ok(MclDomainValue::Unit)
}
pub fn realtime_load(world: &mut World, agent_id: &ResourceId) -> Result<MclDomainValue, MclError> {
    let entity = world
        .entity_by_resource_id(agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    let memory = world
        .get_component::<Agent>(entity)
        .map(|agent| agent.memory.clone())
        .ok_or(MclError::AgentRuntimeMissing)?;
    Ok(MclDomainValue::Inner(BlockInner::Message(
        memory
            .read_realtime()
            .map_err(|_| MclError::RealtimeReadFailed)?
            .into_iter()
            .filter(|message| !matches!(message.message, margatroid_types::Message::System { .. }))
            .collect(),
    )))
}
pub fn domain_to_command(value: MclDomainValue) -> MclCommandValue {
    value
}
