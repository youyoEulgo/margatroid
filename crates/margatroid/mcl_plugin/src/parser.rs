use std::collections::BTreeSet;

use margatroid_types::block_types::{AgentBlock, BlockInner, BlockPath, InnerType};
use margatroid_types::message_types::AgentMessage;
use resource_id_plugin::ResourceID;
use serde_json::Value;

use crate::MclError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedOperation {
    Create {
        block_id: String,
        temporary: bool,
        fields: Vec<FieldDeclaration>,
    },
    Get {
        expression: ArrayExpression,
    },
    Inject {
        source: ArrayParameter,
        target: Selector,
    },
    Import {
        kind: InnerType,
        resource_id: ResourceID,
        alias: String,
    },
    Effect {
        name: String,
        parameters: Vec<ArrayParameter>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDeclaration {
    pub inner_id: String,
    pub kind: InnerType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrayExpression {
    pub sources: Vec<ArraySource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArraySource {
    Alias(String),
    Selector(Selector),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArrayParameter {
    Expression(ArrayExpression),
    Manual { index: usize },
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub path: BlockPath,
    pub range: Range,
    pub position: SelectorPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectorPosition {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    All,
    Empty,
    Single(i64),
    Range(Endpoint, Endpoint),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Value(i64),
    Open,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandardSelector {
    pub path: BlockPath,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedInject {
    pub path: BlockPath,
    pub start: usize,
    pub end: usize,
    pub replacement: BlockInner,
}

pub fn parse(text: &str, arguments: &[Value]) -> Result<ParsedOperation, MclError> {
    if text.contains(';') {
        return Err(MclError::InvalidCommand);
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let (verb, body) = take_word(&normalized);
    let operation = match verb {
        "CREATE" => parse_create(body)?,
        "GET" => ParsedOperation::Get {
            expression: parse_array_expression(body)?,
        },
        "INJECT" => parse_inject(body)?,
        "IMPORT" => {
            let words = body.split_whitespace().collect::<Vec<_>>();
            let [kind, resource, "AS", alias] = words.as_slice() else {
                return Err(MclError::InvalidCommand);
            };
            validate_alias(alias)?;
            let kind = parse_kind(kind)?;
            let resource_id: ResourceID =
                resource.parse().map_err(|_| MclError::InvalidResourceId)?;
            validate_import_kind(kind, &resource_id)?;
            ParsedOperation::Import {
                kind,
                resource_id,
                alias: (*alias).into(),
            }
        }
        "EMIT" => {
            let (keyword, tail) = take_word(body);
            if keyword != "EFFECT" {
                return Err(MclError::InvalidCommand);
            }
            parse_effect(tail)?
        }
        _ => return Err(MclError::InvalidCommand),
    };
    let manual_count = match &operation {
        ParsedOperation::Inject {
            source: ArrayParameter::Manual { .. },
            ..
        } => 1,
        ParsedOperation::Effect { parameters, .. } => parameters
            .iter()
            .filter(|p| matches!(p, ArrayParameter::Manual { .. }))
            .count(),
        _ => 0,
    };
    validate_arguments(arguments, manual_count)?;
    Ok(operation)
}

fn take_word(text: &str) -> (&str, &str) {
    let text = text.trim();
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim()),
        None => (text, ""),
    }
}

pub fn validate_identifier(value: &str) -> Result<(), MclError> {
    let mut bytes = value.bytes();
    let valid_first = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_');
    if !valid_first || !bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return Err(MclError::InvalidCommand);
    }
    Ok(())
}

fn parse_create(body: &str) -> Result<ParsedOperation, MclError> {
    let (keyword, tail) = take_word(body);
    let temporary = keyword == "TEMP";
    let (keyword, tail) = if temporary {
        take_word(tail)
    } else {
        (keyword, tail)
    };
    if keyword != "BLOCK" {
        return Err(MclError::InvalidCommand);
    }
    let open = tail.find('(').ok_or(MclError::InvalidCommand)?;
    let block_id = tail[..open].trim();
    validate_identifier(block_id)?;
    let fields_text = tail[open + 1..]
        .strip_suffix(')')
        .ok_or(MclError::InvalidCommand)?
        .trim();
    if fields_text.contains(['(', ')', '[', ']']) {
        return Err(MclError::InvalidCommand);
    }
    let mut fields = Vec::new();
    let mut names = BTreeSet::new();
    if !fields_text.is_empty() {
        for group in fields_text.split(',') {
            let words = group.split_whitespace().collect::<Vec<_>>();
            if words.is_empty() || words.len() % 2 != 0 {
                return Err(MclError::InvalidCommand);
            }
            for pair in words.chunks_exact(2) {
                validate_identifier(pair[0])?;
                if !names.insert(pair[0]) {
                    return Err(MclError::InvalidCommand);
                }
                let kind = match pair[1] {
                    "MESSAGE" => InnerType::Message,
                    "TOOL" => InnerType::Tool,
                    "SANDBOX" => InnerType::Sandbox,
                    _ => return Err(MclError::InvalidCommand),
                };
                fields.push(FieldDeclaration {
                    inner_id: pair[0].into(),
                    kind,
                });
            }
        }
    }
    Ok(ParsedOperation::Create {
        block_id: block_id.into(),
        temporary,
        fields,
    })
}

pub fn parse_array_expression(text: &str) -> Result<ArrayExpression, MclError> {
    let mut start = 0;
    let mut in_bracket = false;
    let mut sources = Vec::new();
    for (index, character) in text.char_indices() {
        match character {
            '[' if !in_bracket => in_bracket = true,
            ']' if in_bracket => in_bracket = false,
            '[' | ']' | '(' | ')' | '?' => return Err(MclError::InvalidCommand),
            ',' if !in_bracket => {
                sources.push(parse_source(&text[start..index])?);
                start = index + 1;
            }
            _ => {}
        }
    }
    if in_bracket {
        return Err(MclError::InvalidCommand);
    }
    sources.push(parse_source(&text[start..])?);
    Ok(ArrayExpression { sources })
}

pub fn parse_selector(text: &str, position: SelectorPosition) -> Result<Selector, MclError> {
    let text = text.trim();
    let (path, range) = match text.find('[') {
        None => (text, Range::All),
        Some(index) => {
            let suffix = text[index + 1..]
                .strip_suffix(']')
                .ok_or(MclError::InvalidCommand)?;
            if suffix.contains(['[', ']']) {
                return Err(MclError::InvalidCommand);
            }
            let words = suffix.split(',').collect::<Vec<_>>();
            let range = match words.as_slice() {
                [one] if one.trim().is_empty() => Range::Empty,
                [one] => Range::Single(parse_index(one)?),
                [left, right] => Range::Range(parse_endpoint(left)?, parse_endpoint(right)?),
                _ => return Err(MclError::InvalidCommand),
            };
            (text[..index].trim(), range)
        }
    };
    let (block_id, inner_id) = path.split_once('.').ok_or(MclError::InvalidCommand)?;
    validate_identifier(block_id)?;
    validate_identifier(inner_id)?;
    Ok(Selector {
        path: BlockPath {
            block_id: block_id.into(),
            inner_id: inner_id.into(),
        },
        range,
        position,
    })
}

fn parse_index(value: &str) -> Result<i64, MclError> {
    value.trim().parse().map_err(|_| MclError::InvalidCommand)
}

fn parse_endpoint(value: &str) -> Result<Endpoint, MclError> {
    if value.trim().is_empty() {
        Ok(Endpoint::Open)
    } else {
        Ok(Endpoint::Value(parse_index(value)?))
    }
}

fn parse_inject(body: &str) -> Result<ParsedOperation, MclError> {
    let mut in_bracket = false;
    let mut delimiter = None;
    for (index, character) in body.char_indices() {
        match character {
            '[' if !in_bracket => in_bracket = true,
            ']' if in_bracket => in_bracket = false,
            '[' | ']' | '(' | ')' => return Err(MclError::InvalidCommand),
            'T' if !in_bracket && body[index..].starts_with("TO") => {
                let before = &body[..index];
                let after = &body[index + 2..];
                if before.ends_with(char::is_whitespace) && after.starts_with(char::is_whitespace) {
                    if delimiter.replace(index).is_some() {
                        return Err(MclError::InvalidCommand);
                    }
                }
            }
            _ => {}
        }
    }
    if in_bracket {
        return Err(MclError::InvalidCommand);
    }
    let index = delimiter.ok_or(MclError::InvalidCommand)?;
    let source = match body[..index].trim() {
        "?" => ArrayParameter::Manual { index: 0 },
        "[]" => ArrayParameter::Empty,
        text => ArrayParameter::Expression(parse_array_expression(text)?),
    };
    let target = parse_selector(&body[index + 2..], SelectorPosition::Write)?;
    Ok(ParsedOperation::Inject { source, target })
}

fn parse_effect(body: &str) -> Result<ParsedOperation, MclError> {
    let (name, mut tail) = take_word(body);
    let expected = effect_types(name)?;
    let mut parameters = Vec::new();
    let mut manual_index = 0;
    while !tail.is_empty() {
        if let Some(rest) = tail.strip_prefix('?') {
            if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
                return Err(MclError::InvalidCommand);
            }
            parameters.push(ArrayParameter::Manual {
                index: manual_index,
            });
            manual_index += 1;
            tail = rest.trim_start();
        } else if let Some(rest) = tail.strip_prefix('(') {
            let close = rest.find(')').ok_or(MclError::InvalidCommand)?;
            parameters.push(ArrayParameter::Expression(parse_array_expression(
                &rest[..close],
            )?));
            tail = rest[close + 1..].trim_start();
        } else {
            return Err(MclError::InvalidCommand);
        }
    }
    if parameters.len() != expected.len() {
        return Err(MclError::EffectInvalid);
    }
    Ok(ParsedOperation::Effect {
        name: name.into(),
        parameters,
    })
}

fn validate_arguments(arguments: &[Value], expected: usize) -> Result<(), MclError> {
    if arguments.len() != expected {
        return Err(MclError::BindingMissing);
    }
    if arguments.iter().any(|value| !value.is_array()) {
        return Err(MclError::TypeMismatch);
    }
    Ok(())
}

pub fn effect_types(name: &str) -> Result<&'static [InnerType], MclError> {
    match name {
        "wake" | "ready" | "start_turn" | "finish_turn" => Ok(&[]),
        "inference" => Ok(&[InnerType::Message, InnerType::Tool]),
        "tool_call" => Ok(&[InnerType::Message, InnerType::Tool]),
        "history_append" => Ok(&[InnerType::Message]),
        "sandbox_use" => Ok(&[InnerType::Sandbox]),
        _ => Err(MclError::EffectInvalid),
    }
}

pub fn map_selector(selector: &Selector, length: usize) -> Result<StandardSelector, MclError> {
    let len = i64::try_from(length).map_err(|_| MclError::TypeMismatch)?;
    let offset = len.checked_add(1).ok_or(MclError::TypeMismatch)?;
    let mapped = |value: i64| -> Result<i64, MclError> {
        if value >= 0 {
            Ok(value)
        } else {
            offset.checked_add(value).ok_or(MclError::TypeMismatch)
        }
    };
    let (start, end) = match selector.range {
        Range::All => (0, len),
        Range::Empty => (0, 0),
        Range::Single(0) if length == 0 && selector.position == SelectorPosition::Read => (0, 0),
        Range::Single(index) => {
            let index_mapped = mapped(index)?;
            match selector.position {
                SelectorPosition::Write => (index_mapped, index_mapped),
                SelectorPosition::Read if index >= 0 => (
                    index_mapped,
                    index_mapped.checked_add(1).ok_or(MclError::TypeMismatch)?,
                ),
                SelectorPosition::Read => (
                    index_mapped.checked_sub(1).ok_or(MclError::TypeMismatch)?,
                    index_mapped,
                ),
            }
        }
        Range::Range(Endpoint::Open, Endpoint::Open) => return Err(MclError::InvalidCommand),
        Range::Range(Endpoint::Value(left), Endpoint::Value(right))
            if (left < 0) != (right < 0) =>
        {
            return Err(MclError::InvalidCommand)
        }
        Range::Range(left, right) => (
            match left {
                Endpoint::Open => 0,
                Endpoint::Value(value) => mapped(value)?,
            },
            match right {
                Endpoint::Open => len,
                Endpoint::Value(value) => mapped(value)?,
            },
        ),
    };
    if start < 0 || end < start || end > len {
        return Err(MclError::TypeMismatch);
    }
    Ok(StandardSelector {
        path: selector.path.clone(),
        start: start as usize,
        end: end as usize,
    })
}

pub fn select_array(
    blocks: &AgentBlock,
    expression: &ArrayExpression,
) -> Result<BlockInner, MclError> {
    select_array_with(blocks, expression, &|_| Err(MclError::AliasMissing))
}

pub fn select_array_with(
    blocks: &AgentBlock,
    expression: &ArrayExpression,
    resolve: &impl Fn(&str) -> Result<BlockInner, MclError>,
) -> Result<BlockInner, MclError> {
    let mut result: Option<BlockInner> = None;
    for source in &expression.sources {
        let selected = match source {
            ArraySource::Alias(alias) => resolve(alias)?,
            ArraySource::Selector(selector) => {
                if selector.position != SelectorPosition::Read {
                    return Err(MclError::InvalidCommand);
                }
                let inner = read_field(blocks, &selector.path)?;
                let standard = map_selector(selector, inner.len())?;
                let selected = inner
                    .slice(standard.start, standard.end)
                    .map_err(|_| MclError::TypeMismatch)?;
                selected
            }
        };
        match &mut result {
            Some(result) => result
                .append(selected)
                .map_err(|_| MclError::TypeMismatch)?,
            None => result = Some(selected),
        }
    }
    result.ok_or(MclError::InvalidCommand)
}

fn read_field<'a>(blocks: &'a AgentBlock, path: &BlockPath) -> Result<&'a BlockInner, MclError> {
    blocks.get(path).map_err(|_| MclError::SourceReadFailed)
}

pub fn message_from_lua_json(value: Value) -> Result<AgentMessage, MclError> {
    serde_json::from_value(value).map_err(|_| MclError::TypeMismatch)
}

pub fn decode_manual_array(value: &Value, kind: InnerType) -> Result<BlockInner, MclError> {
    let values = value.as_array().ok_or(MclError::TypeMismatch)?;
    match kind {
        InnerType::Message => values
            .iter()
            .cloned()
            .map(message_from_lua_json)
            .collect::<Result<Vec<_>, _>>()
            .map(BlockInner::Message),
        InnerType::Tool | InnerType::Sandbox if values.is_empty() => Ok(BlockInner::empty(kind)),
        InnerType::Tool | InnerType::Sandbox => Err(MclError::TypeMismatch),
    }
}

fn evaluate_parameter(
    resolve: &impl Fn(&str) -> Result<BlockInner, MclError>,
    blocks: &AgentBlock,
    parameter: &ArrayParameter,
    arguments: &[Value],
    expected: InnerType,
) -> Result<BlockInner, MclError> {
    let result = match parameter {
        ArrayParameter::Expression(expression) => select_array_with(blocks, expression, resolve)?,
        ArrayParameter::Manual { index } => {
            let value = arguments.get(*index).ok_or(MclError::BindingMissing)?;
            if expected == InnerType::Message {
                decode_manual_array(value, expected)?
            } else {
                let aliases = value.as_array().ok_or(MclError::TypeMismatch)?;
                let mut result = BlockInner::empty(expected);
                for alias in aliases {
                    let alias = alias.as_str().ok_or(MclError::TypeMismatch)?;
                    validate_alias(alias)?;
                    result
                        .append(resolve(alias)?)
                        .map_err(|_| MclError::TypeMismatch)?;
                }
                result
            }
        }
        ArrayParameter::Empty => BlockInner::empty(expected),
    };
    if result.inner_type() != expected {
        return Err(MclError::TypeMismatch);
    }
    Ok(result)
}

pub fn prepare_inject_with(
    resolve: &impl Fn(&str) -> Result<BlockInner, MclError>,
    blocks: &AgentBlock,
    source: &ArrayParameter,
    target: &Selector,
    arguments: &[Value],
) -> Result<PreparedInject, MclError> {
    if target.position != SelectorPosition::Write {
        return Err(MclError::InvalidCommand);
    }
    validate_arguments(
        arguments,
        usize::from(matches!(source, ArrayParameter::Manual { index: 0 })),
    )?;
    let inner = read_field(blocks, &target.path)?;
    let replacement = evaluate_parameter(resolve, blocks, source, arguments, inner.inner_type())?;
    let target = map_selector(target, inner.len())?;
    Ok(PreparedInject {
        path: target.path,
        start: target.start,
        end: target.end,
        replacement,
    })
}

pub fn prepare_inject(
    blocks: &AgentBlock,
    source: &ArrayParameter,
    target: &Selector,
    arguments: &[Value],
) -> Result<PreparedInject, MclError> {
    prepare_inject_with(
        &|_| Err(MclError::AliasMissing),
        blocks,
        source,
        target,
        arguments,
    )
}

pub fn apply_inject(
    blocks: &mut AgentBlock,
    source: &ArrayParameter,
    target: &Selector,
    arguments: &[Value],
) -> Result<(), MclError> {
    let prepared = prepare_inject(blocks, source, target, arguments)?;
    blocks
        .replace(
            &prepared.path,
            prepared.start,
            prepared.end,
            prepared.replacement,
        )
        .map_err(|_| MclError::TypeMismatch)
}

pub fn create_block(
    blocks: &mut AgentBlock,
    block_id: &str,
    fields: &[FieldDeclaration],
) -> Result<(), MclError> {
    validate_identifier(block_id)?;
    let mut names = BTreeSet::new();
    for field in fields {
        validate_identifier(&field.inner_id)?;
        if !names.insert(&field.inner_id) {
            return Err(MclError::InvalidCommand);
        }
    }
    blocks
        .create(
            block_id.into(),
            fields
                .iter()
                .map(|f| (f.inner_id.clone(), f.kind))
                .collect(),
        )
        .map_err(|_| MclError::InvalidCommand)
}

pub fn evaluate_effect_arguments_with(
    resolve: &impl Fn(&str) -> Result<BlockInner, MclError>,
    blocks: &AgentBlock,
    name: &str,
    parameters: &[ArrayParameter],
    arguments: &[Value],
) -> Result<Vec<BlockInner>, MclError> {
    let expected = effect_types(name)?;
    if parameters.len() != expected.len() {
        return Err(MclError::EffectInvalid);
    }
    let mut manual_count = 0;
    for parameter in parameters {
        match parameter {
            ArrayParameter::Manual { index } if *index == manual_count => manual_count += 1,
            ArrayParameter::Manual { .. } | ArrayParameter::Empty => {
                return Err(MclError::InvalidCommand)
            }
            ArrayParameter::Expression(_) => {}
        }
    }
    validate_arguments(arguments, manual_count)?;
    let arrays = parameters
        .iter()
        .zip(expected)
        .map(|(parameter, kind)| evaluate_parameter(resolve, blocks, parameter, arguments, *kind))
        .collect::<Result<Vec<_>, _>>()?;
    if name == "sandbox_use" && arrays[0].len() > 1 {
        return Err(MclError::EffectInvalid);
    }
    if name == "tool_call" {
        validate_tool_parameters(&arrays)?;
    }
    Ok(arrays)
}

pub fn validate_alias(value: &str) -> Result<(), MclError> {
    validate_identifier(value)?;
    if value.contains('.') {
        return Err(MclError::InvalidCommand);
    }
    Ok(())
}

fn parse_kind(value: &str) -> Result<InnerType, MclError> {
    match value {
        "MESSAGE" => Ok(InnerType::Message),
        "TOOL" => Ok(InnerType::Tool),
        "SANDBOX" => Ok(InnerType::Sandbox),
        _ => Err(MclError::InvalidCommand),
    }
}

pub fn validate_import_kind(kind: InnerType, id: &ResourceID) -> Result<(), MclError> {
    if matches!(
        (kind, id.resource_type()),
        (InnerType::Message, "prompt")
            | (InnerType::Tool, "tool")
            | (InnerType::Sandbox, "sandbox")
    ) {
        Ok(())
    } else {
        Err(MclError::ImportKindMismatch)
    }
}

fn parse_source(text: &str) -> Result<ArraySource, MclError> {
    let text = text.trim();
    if text.contains('.') {
        parse_selector(text, SelectorPosition::Read).map(ArraySource::Selector)
    } else {
        validate_alias(text)?;
        Ok(ArraySource::Alias(text.into()))
    }
}

pub fn validate_tool_parameters(arrays: &[BlockInner]) -> Result<(), MclError> {
    if arrays.len() != 2 {
        return Err(MclError::EffectInvalid);
    }
    let BlockInner::Message(messages) = &arrays[0] else {
        return Err(MclError::TypeMismatch);
    };
    let [AgentMessage::Assistant { tool_calls, .. }] = messages.as_slice() else {
        return Err(MclError::ToolCallInvalid);
    };
    let BlockInner::Tool(visible) = &arrays[1] else {
        return Err(MclError::TypeMismatch);
    };
    let mut ids = BTreeSet::new();
    for call in tool_calls {
        if call.id.trim().is_empty() || call.tool_name.trim().is_empty() || !ids.insert(&call.id) {
            return Err(MclError::ToolCallInvalid);
        }
    }
    let mut aliases = BTreeSet::new();
    for resource in visible {
        if !aliases.insert(&resource.alias) {
            return Err(MclError::AliasConflict);
        }
    }
    Ok(())
}

pub fn evaluate_effect_arguments(
    blocks: &AgentBlock,
    name: &str,
    parameters: &[ArrayParameter],
    arguments: &[Value],
) -> Result<Vec<BlockInner>, MclError> {
    evaluate_effect_arguments_with(
        &|_| Err(MclError::AliasMissing),
        blocks,
        name,
        parameters,
        arguments,
    )
}
