use std::collections::{BTreeMap, BTreeSet};

use margatroid_types::ResourceId;

use crate::{MclError, MclErrorKind, MclProgramKind, MclSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclBlockType {
    Entry,
    Message,
    Resource,
    ToolExchange,
    Context,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclBlockLifetime {
    Persistent,
    Session,
    Turn,
    Request,
}

#[derive(Clone, Debug)]
pub struct MclBlockDefinition {
    pub name: String,
    pub item_type: MclBlockType,
    pub lifetime: MclBlockLifetime,
    pub mutable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclViewKind {
    Messages { blocks: Vec<String> },
    Tools,
    System,
}

#[derive(Clone, Debug)]
pub struct MclViewDefinition {
    pub name: String,
    pub kind: MclViewKind,
}

#[derive(Clone, Debug)]
pub struct MclRequestDefinition {
    pub name: String,
    pub system: String,
    pub messages: String,
    pub tools: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclPredicate {
    Always,
    ToolCallsEmpty,
    ToolCallsNotEmpty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclResourceExpression {
    EventResource,
    Literal(ResourceId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclStatement {
    AppendEntry { block: String },
    AppendExchange { block: String },
    AppendEntryToExchange,
    ClearBlock { block: String },
    RestoreDefaultCapabilities,
    ShowResource { resource: MclResourceExpression },
    HideResource { resource: MclResourceExpression },
    ClearCapabilities,
    EmitInference { request: String },
    EmitTools,
    FinishTurn,
}

#[derive(Clone, Debug)]
pub struct MclHandler {
    pub event: String,
    pub priority: u32,
    pub predicate: MclPredicate,
    pub statements: Vec<MclStatement>,
}

pub(crate) struct MclAst {
    pub imports: Vec<ResourceId>,
    pub kind: MclProgramKind,
    pub name: String,
    pub blocks: Vec<MclBlockDefinition>,
    pub views: Vec<MclViewDefinition>,
    pub requests: Vec<MclRequestDefinition>,
    pub handlers: Vec<MclHandler>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    String(String),
    Symbol(char),
}

pub(crate) fn scan_imports(source: &str) -> Result<Vec<ResourceId>, MclError> {
    let tokens = lex(source)?;
    let mut cursor = Cursor::new(tokens);
    let mut imports = Vec::new();
    while cursor.peek_word("import") {
        cursor.word("import")?;
        imports.push(parse_resource_id(cursor.take_value()?)?);
        if cursor.peek_word("as") {
            cursor.word("as")?;
            cursor.take_word()?;
        }
        cursor.symbol(';')?;
    }
    Ok(imports)
}

pub(crate) fn parse(source: &MclSource) -> Result<MclAst, MclError> {
    let tokens = lex(source.source())?;
    let mut cursor = Cursor::new(tokens);
    let mut imports = Vec::new();
    while cursor.peek_word("import") {
        cursor.word("import")?;
        imports.push(parse_resource_id(cursor.take_value()?)?);
        if cursor.peek_word("as") {
            cursor.word("as")?;
            cursor.take_word()?;
        }
        cursor.symbol(';')?;
    }

    let kind_word = cursor.take_word()?;
    let kind = match kind_word.as_str() {
        "base" => {
            cursor.word("context")?;
            MclProgramKind::Base
        }
        "workflow" => MclProgramKind::Workflow,
        "module" => MclProgramKind::Module,
        _ => return cursor.error("MCL source must declare base context, workflow, or module"),
    };
    let name = cursor.take_word()?;
    cursor.symbol('{')?;

    let mut blocks = Vec::new();
    let mut views = Vec::new();
    let mut requests = Vec::new();
    let mut handlers = Vec::new();
    while !cursor.peek_symbol('}') {
        let exported = if cursor.peek_word("export") {
            cursor.word("export")?;
            true
        } else {
            false
        };
        if exported && kind != MclProgramKind::Module {
            return cursor.error("only module declarations can use export");
        }
        if cursor.peek_word("block") {
            blocks.push(parse_block(&mut cursor)?);
        } else if cursor.peek_word("view") {
            views.push(parse_view(&mut cursor)?);
        } else if cursor.peek_word("request") {
            requests.push(parse_request(&mut cursor)?);
        } else if cursor.peek_word("on") {
            handlers.push(parse_handler(&mut cursor)?);
        } else {
            return cursor.error("unsupported MCL top-level declaration");
        }
    }
    cursor.symbol('}')?;
    if !cursor.is_empty() {
        return cursor.error("unexpected tokens after MCL program");
    }
    Ok(MclAst {
        imports,
        kind,
        name,
        blocks,
        views,
        requests,
        handlers,
    })
}

fn parse_block(cursor: &mut Cursor) -> Result<MclBlockDefinition, MclError> {
    cursor.word("block")?;
    let name = cursor.take_word()?;
    cursor.symbol(':')?;
    let item_type = match cursor.take_word()?.as_str() {
        "entry" => MclBlockType::Entry,
        "message" => MclBlockType::Message,
        "resource" => MclBlockType::Resource,
        "tool_exchange" => MclBlockType::ToolExchange,
        "context" => MclBlockType::Context,
        _ => return cursor.error("unknown MCL Block item type"),
    };
    let mut lifetime = None;
    let mut mutable = true;
    while !cursor.peek_symbol(';') {
        let modifier = cursor.take_word()?;
        match modifier.as_str() {
            "persistent" => lifetime = Some(MclBlockLifetime::Persistent),
            "session" => lifetime = Some(MclBlockLifetime::Session),
            "turn" => lifetime = Some(MclBlockLifetime::Turn),
            "request" => lifetime = Some(MclBlockLifetime::Request),
            "mutable" => mutable = true,
            "immutable" => mutable = false,
            "ordered" => {
                cursor.word("by")?;
                cursor.take_word()?;
            }
            "unique" => {
                cursor.word("by")?;
                cursor.take_word()?;
            }
            _ => return cursor.error("unknown MCL Block modifier"),
        }
    }
    cursor.symbol(';')?;
    Ok(MclBlockDefinition {
        name,
        item_type,
        lifetime: lifetime.unwrap_or(MclBlockLifetime::Persistent),
        mutable,
    })
}

fn parse_view(cursor: &mut Cursor) -> Result<MclViewDefinition, MclError> {
    cursor.word("view")?;
    let name = cursor.take_word()?;
    let declared_type = if cursor.peek_symbol(':') {
        cursor.symbol(':')?;
        Some(cursor.take_word()?)
    } else {
        None
    };
    cursor.symbol('{')?;
    let mut blocks = Vec::new();
    let mut kind = None;
    if cursor.peek_word("union") {
        cursor.word("union")?;
        while !cursor.peek_symbol(';') && !cursor.peek_word("order") {
            blocks.push(cursor.take_word()?);
            if cursor.peek_symbol(',') {
                cursor.symbol(',')?;
            } else {
                break;
            }
        }
        kind = Some(MclViewKind::Messages { blocks });
    } else if cursor.peek_word("select") {
        cursor.word("select")?;
        let mut selected = cursor.take_word()?;
        if selected == "latest" {
            cursor.word("entry")?;
            selected = "entry".to_owned();
        }
        cursor.word("from")?;
        let source = cursor.take_word()?;
        kind = Some(match (selected.as_str(), source.as_str()) {
            ("resource", "capabilities.dynamic") => MclViewKind::Tools,
            ("entry", _) => MclViewKind::Messages {
                blocks: vec![source],
            },
            _ if declared_type.as_deref() == Some("system") => MclViewKind::System,
            _ => return cursor.error("unsupported MCL View selection"),
        });
    }
    skip_balanced_body(cursor)?;
    let kind = kind.or_else(|| match declared_type.as_deref() {
        Some("tools") => Some(MclViewKind::Tools),
        Some("system") => Some(MclViewKind::System),
        _ => None,
    });
    Ok(MclViewDefinition {
        name,
        kind: kind.ok_or_else(|| {
            MclError::new(
                MclErrorKind::ParseFailed,
                "MCL View has no supported source",
            )
        })?,
    })
}

fn skip_balanced_body(cursor: &mut Cursor) -> Result<(), MclError> {
    let mut depth = 1usize;
    while depth > 0 {
        match cursor.take()? {
            Token::Symbol('{') => depth += 1,
            Token::Symbol('}') => depth -= 1,
            _ => {}
        }
    }
    Ok(())
}

fn parse_request(cursor: &mut Cursor) -> Result<MclRequestDefinition, MclError> {
    cursor.word("request")?;
    let name = cursor.take_word()?;
    cursor.symbol('{')?;
    let mut values = BTreeMap::new();
    while !cursor.peek_symbol('}') {
        let key = cursor.take_word()?;
        cursor.symbol('=')?;
        let value = cursor.take_word()?;
        cursor.symbol(';')?;
        values.insert(key, value);
    }
    cursor.symbol('}')?;
    Ok(MclRequestDefinition {
        name,
        system: values.remove("system").ok_or_else(|| {
            MclError::new(MclErrorKind::ParseFailed, "request.system is required")
        })?,
        messages: values.remove("messages").ok_or_else(|| {
            MclError::new(MclErrorKind::ParseFailed, "request.messages is required")
        })?,
        tools: values
            .remove("tools")
            .ok_or_else(|| MclError::new(MclErrorKind::ParseFailed, "request.tools is required"))?,
    })
}

fn parse_handler(cursor: &mut Cursor) -> Result<MclHandler, MclError> {
    cursor.word("on")?;
    let event = cursor.take_word()?;
    if cursor.peek_word("as") {
        cursor.word("as")?;
        cursor.word("event")?;
    }
    let mut priority = 100u32;
    if cursor.peek_word("priority") {
        cursor.word("priority")?;
        priority = cursor
            .take_word()?
            .parse()
            .map_err(|_| MclError::new(MclErrorKind::ParseFailed, "priority must be u32"))?;
    }
    let mut predicate = MclPredicate::Always;
    if cursor.peek_word("where") {
        cursor.word("where")?;
        cursor.word("event.tool_calls")?;
        cursor.word("is")?;
        if cursor.peek_word("not") {
            cursor.word("not")?;
            predicate = MclPredicate::ToolCallsNotEmpty;
        } else {
            predicate = MclPredicate::ToolCallsEmpty;
        }
        cursor.word("empty")?;
    }
    if cursor.peek_word("transaction") {
        cursor.word("transaction")?;
    }
    cursor.symbol('{')?;
    let mut statements = Vec::new();
    while !cursor.peek_symbol('}') {
        statements.push(parse_statement(cursor)?);
    }
    cursor.symbol('}')?;
    Ok(MclHandler {
        event,
        priority,
        predicate,
        statements,
    })
}

fn parse_statement(cursor: &mut Cursor) -> Result<MclStatement, MclError> {
    let operation = cursor.take_word()?;
    let statement = match operation.as_str() {
        "append" => {
            let value = cursor.take_word()?;
            cursor.word("into")?;
            let target = cursor.take_word()?;
            match (value.as_str(), target.as_str()) {
                ("event.entry", "event.exchange") => MclStatement::AppendEntryToExchange,
                ("event.entry", _) => MclStatement::AppendEntry { block: target },
                ("event.exchange", _) => MclStatement::AppendExchange { block: target },
                _ => return cursor.error("unsupported append statement"),
            }
        }
        "clear" => {
            let target = cursor.take_word()?;
            if target == "capabilities.dynamic" {
                MclStatement::ClearCapabilities
            } else {
                MclStatement::ClearBlock { block: target }
            }
        }
        "restore" => {
            cursor.word("capabilities.dynamic")?;
            cursor.word("from")?;
            cursor.word("capabilities.default")?;
            MclStatement::RestoreDefaultCapabilities
        }
        "show" | "hide" => {
            cursor.word("resource")?;
            let value = cursor.take_value()?;
            let resource = if value == "event.resource" {
                MclResourceExpression::EventResource
            } else {
                MclResourceExpression::Literal(parse_any_resource_id(value)?)
            };
            cursor.word(if operation == "show" { "in" } else { "from" })?;
            cursor.word("capabilities.dynamic")?;
            if operation == "show" {
                MclStatement::ShowResource { resource }
            } else {
                MclStatement::HideResource { resource }
            }
        }
        "emit" => match cursor.take_word()?.as_str() {
            "inference" => {
                cursor.word("using")?;
                MclStatement::EmitInference {
                    request: cursor.take_word()?,
                }
            }
            "tools" => {
                cursor.word("event.tool_calls")?;
                MclStatement::EmitTools
            }
            _ => return cursor.error("unsupported emit statement"),
        },
        "finish" => {
            cursor.word("turn")?;
            MclStatement::FinishTurn
        }
        _ => return cursor.error("unsupported MCL statement"),
    };
    cursor.symbol(';')?;
    Ok(statement)
}

pub(crate) fn validate(
    blocks: &[MclBlockDefinition],
    views: &[MclViewDefinition],
    requests: &[MclRequestDefinition],
    handlers: &[MclHandler],
) -> Result<(), MclError> {
    unique_names(blocks.iter().map(|value| value.name.as_str()), "Block")?;
    unique_names(views.iter().map(|value| value.name.as_str()), "View")?;
    unique_names(requests.iter().map(|value| value.name.as_str()), "Request")?;
    let block_names = blocks
        .iter()
        .map(|value| value.name.as_str())
        .collect::<BTreeSet<_>>();
    let block_definitions = blocks
        .iter()
        .map(|block| (block.name.as_str(), block))
        .collect::<BTreeMap<_, _>>();
    for view in views {
        if let MclViewKind::Messages { blocks } = &view.kind {
            for block in blocks {
                if !block_names.contains(block.as_str()) {
                    return Err(MclError::new(
                        MclErrorKind::TypeMismatch,
                        format!("View {} references unknown Block {block}", view.name),
                    ));
                }
            }
        }
    }
    for request in requests {
        if request.system != "agent.system"
            && !views
                .iter()
                .any(|view| view.name == request.system && matches!(view.kind, MclViewKind::System))
        {
            return Err(MclError::new(
                MclErrorKind::TypeMismatch,
                format!("Request {} references unknown System View", request.name),
            ));
        }
        if !views.iter().any(|view| {
            view.name == request.messages && matches!(view.kind, MclViewKind::Messages { .. })
        }) || !views
            .iter()
            .any(|view| view.name == request.tools && matches!(view.kind, MclViewKind::Tools))
        {
            return Err(MclError::new(
                MclErrorKind::TypeMismatch,
                format!("Request {} references an unknown View", request.name),
            ));
        }
    }
    let request_names = requests
        .iter()
        .map(|value| value.name.as_str())
        .collect::<BTreeSet<_>>();
    for handler in handlers {
        if !SUPPORTED_EVENTS.contains(&handler.event.as_str()) {
            return Err(MclError::new(
                MclErrorKind::InvalidEvent,
                format!("Handler references unsupported event {}", handler.event),
            ));
        }
        for statement in &handler.statements {
            match statement {
                MclStatement::AppendEntry { block }
                | MclStatement::AppendExchange { block }
                | MclStatement::ClearBlock { block }
                    if !block_names.contains(block.as_str()) =>
                {
                    return Err(MclError::new(
                        MclErrorKind::TypeMismatch,
                        format!("Handler references unknown Block {block}"),
                    ));
                }
                MclStatement::AppendEntry { block } => {
                    let definition = block_definitions[block.as_str()];
                    if !definition.mutable
                        || !matches!(
                            definition.item_type,
                            MclBlockType::Entry | MclBlockType::Message | MclBlockType::Context
                        )
                    {
                        return Err(MclError::new(
                            MclErrorKind::TypeMismatch,
                            format!("Block {block} cannot accept message entries"),
                        ));
                    }
                }
                MclStatement::AppendExchange { block } => {
                    let definition = block_definitions[block.as_str()];
                    if !definition.mutable
                        || !matches!(
                            definition.item_type,
                            MclBlockType::ToolExchange | MclBlockType::Context
                        )
                    {
                        return Err(MclError::new(
                            MclErrorKind::TypeMismatch,
                            format!("Block {block} cannot accept ToolExchanges"),
                        ));
                    }
                }
                MclStatement::ClearBlock { block }
                    if !block_definitions[block.as_str()].mutable =>
                {
                    return Err(MclError::new(
                        MclErrorKind::TypeMismatch,
                        format!("immutable Block {block} cannot be cleared"),
                    ));
                }
                MclStatement::EmitInference { request }
                    if !request_names.contains(request.as_str()) =>
                {
                    return Err(MclError::new(
                        MclErrorKind::TypeMismatch,
                        format!("Handler references unknown Request {request}"),
                    ));
                }
                _ => {}
            }
        }
    }
    validate_write_conflicts(handlers)?;
    Ok(())
}

const SUPPORTED_EVENTS: &[&str] = &[
    "agent.created",
    "message.user",
    "message.assistant",
    "message.tool",
    "tool.batch.completed",
    "tool.batch.failed",
    "inference.failed",
    "turn.aborted",
    "resource.injected",
    "resource.removed",
    "workflow.attached",
    "workflow.detaching",
];

fn validate_write_conflicts(handlers: &[MclHandler]) -> Result<(), MclError> {
    let mut writes = BTreeMap::<(&str, u32, String), Vec<usize>>::new();
    for (index, handler) in handlers.iter().enumerate() {
        let mut handler_writes = BTreeSet::new();
        for statement in &handler.statements {
            let target = match statement {
                MclStatement::AppendEntry { block }
                | MclStatement::AppendExchange { block }
                | MclStatement::ClearBlock { block } => Some(format!("block:{block}")),
                MclStatement::RestoreDefaultCapabilities
                | MclStatement::ShowResource { .. }
                | MclStatement::HideResource { .. }
                | MclStatement::ClearCapabilities => Some("capabilities.dynamic".to_owned()),
                _ => None,
            };
            if let Some(target) = target {
                handler_writes.insert(target);
            }
        }
        for target in handler_writes {
            let key = (handler.event.as_str(), handler.priority, target.clone());
            let conflicting = writes.get(&key).is_some_and(|previous| {
                previous.iter().any(|previous| {
                    !predicates_are_disjoint(&handlers[*previous].predicate, &handler.predicate)
                })
            });
            if conflicting {
                return Err(MclError::new(
                    MclErrorKind::WriteConflict,
                    format!(
                        "Handlers for {} at priority {} both write {target}",
                        handler.event, handler.priority
                    ),
                ));
            }
            writes.entry(key).or_default().push(index);
        }
    }
    Ok(())
}

fn predicates_are_disjoint(left: &MclPredicate, right: &MclPredicate) -> bool {
    matches!(
        (left, right),
        (
            MclPredicate::ToolCallsEmpty,
            MclPredicate::ToolCallsNotEmpty
        ) | (
            MclPredicate::ToolCallsNotEmpty,
            MclPredicate::ToolCallsEmpty
        )
    )
}

fn unique_names<'a>(names: impl Iterator<Item = &'a str>, kind: &str) -> Result<(), MclError> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(MclError::new(
                MclErrorKind::DuplicateName,
                format!("duplicate MCL {kind} name {name}"),
            ));
        }
    }
    Ok(())
}

fn parse_resource_id(value: String) -> Result<ResourceId, MclError> {
    let resource = parse_any_resource_id(value)?;
    if resource.resource_type() != "mcl" {
        return Err(MclError::new(
            MclErrorKind::InvalidResourceId,
            "MCL imports must use type mcl",
        ));
    }
    Ok(resource)
}

fn parse_any_resource_id(value: String) -> Result<ResourceId, MclError> {
    ResourceId::parse(value).map_err(|error| {
        MclError::new(
            MclErrorKind::InvalidResourceId,
            format!("invalid MCL resource ID: {error}"),
        )
    })
}

fn lex(source: &str) -> Result<Vec<Token>, MclError> {
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((_, character)) = chars.next() {
        if character.is_whitespace() {
            continue;
        }
        if character == '#' {
            while chars.next().is_some_and(|(_, value)| value != '\n') {}
            continue;
        }
        if character == '/' && chars.peek().is_some_and(|(_, value)| *value == '/') {
            chars.next();
            while chars.next().is_some_and(|(_, value)| value != '\n') {}
            continue;
        }
        if "{}[];=,:".contains(character) {
            tokens.push(Token::Symbol(character));
            continue;
        }
        if character == '"' {
            let mut value = String::new();
            let mut escaped = false;
            let mut closed = false;
            for (_, next) in chars.by_ref() {
                if escaped {
                    value.push(match next {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    });
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == '"' {
                    closed = true;
                    break;
                } else {
                    value.push(next);
                }
            }
            if !closed {
                return Err(MclError::new(
                    MclErrorKind::ParseFailed,
                    "unterminated MCL string",
                ));
            }
            tokens.push(Token::String(value));
            continue;
        }
        let mut value = String::from(character);
        while let Some((_, next)) = chars.peek().copied() {
            if next.is_whitespace() || "{}[];=,:\"".contains(next) || next == '#' {
                break;
            }
            value.push(next);
            chars.next();
        }
        tokens.push(Token::Word(value));
    }
    Ok(tokens)
}

struct Cursor {
    tokens: Vec<Token>,
    index: usize,
}

impl Cursor {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, index: 0 }
    }

    fn is_empty(&self) -> bool {
        self.index == self.tokens.len()
    }

    fn peek_word(&self, expected: &str) -> bool {
        matches!(self.tokens.get(self.index), Some(Token::Word(value)) if value == expected)
    }

    fn peek_symbol(&self, expected: char) -> bool {
        matches!(self.tokens.get(self.index), Some(Token::Symbol(value)) if *value == expected)
    }

    fn take(&mut self) -> Result<Token, MclError> {
        let token = self.tokens.get(self.index).cloned().ok_or_else(|| {
            MclError::new(MclErrorKind::ParseFailed, "unexpected end of MCL source")
        })?;
        self.index += 1;
        Ok(token)
    }

    fn take_word(&mut self) -> Result<String, MclError> {
        match self.take()? {
            Token::Word(value) => Ok(value),
            _ => self.error("expected MCL identifier"),
        }
    }

    fn take_value(&mut self) -> Result<String, MclError> {
        match self.take()? {
            Token::Word(mut value) => {
                while self.peek_symbol(':') {
                    self.symbol(':')?;
                    value.push(':');
                    value.push_str(&self.take_word()?);
                }
                Ok(value)
            }
            Token::String(value) => Ok(value),
            _ => self.error("expected MCL value"),
        }
    }

    fn word(&mut self, expected: &str) -> Result<(), MclError> {
        let value = self.take_word()?;
        if value == expected {
            Ok(())
        } else {
            self.error(format!("expected {expected}, found {value}"))
        }
    }

    fn symbol(&mut self, expected: char) -> Result<(), MclError> {
        match self.take()? {
            Token::Symbol(value) if value == expected => Ok(()),
            _ => self.error(format!("expected symbol {expected}")),
        }
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, MclError> {
        Err(MclError::new(
            MclErrorKind::ParseFailed,
            format!("{} at token {}", message.into(), self.index),
        ))
    }
}
