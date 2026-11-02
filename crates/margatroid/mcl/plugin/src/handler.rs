use crate::types::*;
use crate::{InstalledMcl, MclImportConfig};
use address::{Address, ParsedAddress, resolve_file};
use agent_core::{Agent, AgentState, WorldAgentExt};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::{Entity, World};
use futures_util::FutureExt;
use inference_core::{
    ActiveInference, AgentInferenceConfig, InferenceMessage, InferenceRequest, InferenceResponse,
    InferenceToolCall,
};
use lua_runtime_core::{DriverLuaVm, WorldLuaRuntimeExt};
use mcl_core::*;
use prompt_resource_core::{ImportPrompt, PromptReadLimits, PromptRole, Prompts, read_prompt};
use sandbox_resource_core::{ImportSandbox, Sandboxes, WorldSandboxResourceExt};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use storage_core::{
    StorageError, StorageErrorKind, StorageOperation, StorageRequest, StorageValue,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_core::{ToolErrorKind, ToolExecuteRequest};
use tool_resource_core::{ImportTool, Tools};

fn error(kind: MclErrorKind, message: impl Into<String>) -> MclError {
    MclError::new(kind, message)
}

fn respond(request: &MclCommand, result: Result<MclValue, MclError>) {
    if let Some(sender) = lock(&request.reply).take() {
        let _ = sender.send(result);
    }
}

fn validate_source(world: &World, request: &MclCommand) -> Result<(), MclError> {
    if !world.is_alive(request.source) {
        return Err(error(MclErrorKind::SourceInvalid, "MCL source is missing"));
    }
    if !world
        .get_component::<Address>(request.source)
        .is_some_and(|address| address.as_str() == request.source_address)
    {
        return Err(error(
            MclErrorKind::SourceInvalid,
            "MCL source address does not match its caller",
        ));
    }
    let agent = world
        .get_component::<Agent>(request.target)
        .ok_or_else(|| error(MclErrorKind::TargetMissing, "MCL target Agent is missing"))?;
    if agent.state == AgentState::Stopping {
        return Err(error(
            MclErrorKind::Closed,
            "Agent is not accepting commands",
        ));
    }
    if matches!(agent.state, AgentState::Starting | AgentState::Restarting)
        && (request.source != request.target || request.source_instance.is_none())
    {
        return Err(error(
            MclErrorKind::SourceInvalid,
            "only the current self Driver may initialize this Agent",
        ));
    }
    if let Some(instance) = &request.source_instance {
        let vm = world
            .get_component::<DriverLuaVm>(request.source)
            .ok_or_else(|| error(MclErrorKind::SourceInvalid, "Driver source VM is missing"))?;
        if vm.instance.owner != request.source || vm.instance.start_id != *instance {
            return Err(error(
                MclErrorKind::SourceInvalid,
                "Driver source instance is stale",
            ));
        }
    }
    Ok(())
}

pub(crate) fn handle_command(world: &mut World, mut request: MclCommand) {
    let Some(sender) = lock(&request.reply).take() else {
        return;
    };
    request.reply = Arc::new(Mutex::new(Some(sender)));
    let accepted = (|| {
        validate_source(world, &request)?;
        if request.id.trim().is_empty() {
            return Err(error(MclErrorKind::ParseFailed, "MCL request id is empty"));
        }
        if request.cancel.is_cancelled() {
            return Err(error(MclErrorKind::Cancelled, "MCL request was cancelled"));
        }
        if !world.contains_component::<MclQueue>(request.target) {
            world.insert_component(request.target, MclQueue::default());
        }
        let queue = world.get_component_mut::<MclQueue>(request.target).unwrap();
        if !queue.accepting || queue.cancel.is_cancelled() {
            return Err(error(MclErrorKind::Closed, "MCL queue is closed"));
        }
        if queue
            .commands
            .iter()
            .any(|pending| pending.id == request.id)
        {
            return Err(error(MclErrorKind::ParseFailed, "duplicate MCL request id"));
        }
        queue.drain.started();
        queue.commands.push_back(request.clone());
        Ok(())
    })();
    if let Err(error) = accepted {
        respond(&request, Err(error));
    }
}

pub(crate) fn handle_queue(world: &mut World, target: Entity) {
    let Some(queue) = world.get_component::<MclQueue>(target) else {
        return;
    };
    if queue.pending.is_some() {
        return;
    }
    let Some(request) = queue.commands.front().cloned() else {
        return;
    };
    if !queue.accepting || queue.cancel.is_cancelled() || request.cancel.is_cancelled() {
        settle_head(
            world,
            &request,
            Err(error(
                MclErrorKind::Cancelled,
                "MCL command cancelled before execution",
            )),
        );
        return;
    }
    if let Err(error) = validate_source(world, &request) {
        settle_head(world, &request, Err(error));
        return;
    }
    let operation = match parse_instruction(&request.instruction) {
        Ok(operation) => operation,
        Err(error) => {
            settle_head(world, &request, Err(error));
            return;
        }
    };
    let settings = world
        .get_resource::<InstalledMcl>()
        .expect("MCL plugin is not installed");
    let schedule = settings.schedule.clone();
    let queue = world.get_component_mut::<MclQueue>(target).unwrap();
    queue.pending = Some(request.id.clone());
    let cancel = queue.cancel.child_token();
    let allowed_imports = queue.allowed_imports.clone();
    let context = CommandContext {
        request,
        operation,
        cancel,
        events: world.event_sender(),
        world: world.closure_sender(&schedule),
        instance: world
            .get_component::<DriverLuaVm>(target)
            .map(|vm| vm.instance.start_id.clone()),
        turn: world
            .get_component::<Agent>(target)
            .and_then(|agent| agent.turn.clone()),
        prompts: world
            .get_component::<Prompts>(target)
            .map(|prompts| {
                prompts
                    .iter()
                    .map(|(alias, path)| (alias.clone(), path.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        imports: world.get_resource::<MclImportConfig>().unwrap().clone(),
        allowed_imports,
    };
    world.send_async_closure(&schedule, move || execute_command(context));
}

pub(crate) fn handle_command_result(
    world: &mut World,
    result: Result<CommandOutput, CommandError>,
) {
    match result {
        Ok(output) => settle_head(world, &output.request, Ok(output.value)),
        Err(failed) => match failed.request {
            Some(request) => settle_head(world, &request, Err(failed.error)),
            None => {
                tracing::error!(error = %failed.error, "MCL worker failed without a completion association")
            }
        },
    }
}

fn settle_head(world: &mut World, request: &MclCommand, result: Result<MclValue, MclError>) {
    let matched = world
        .get_component::<MclQueue>(request.target)
        .is_some_and(|queue| {
            queue.commands.front().is_some_and(|front| {
                front.id == request.id && Arc::ptr_eq(&front.reply, &request.reply)
            }) && queue.pending.as_deref().is_none_or(|id| id == request.id)
        });
    if !matched {
        respond(
            request,
            Err(error(
                MclErrorKind::Closed,
                "MCL queue changed before command completion",
            )),
        );
        return;
    }
    let cleanup_error = result
        .as_ref()
        .err()
        .filter(|error| {
            matches!(
                error.kind,
                MclErrorKind::CleanupFailed | MclErrorKind::CommitUnknown
            )
        })
        .cloned();
    respond(request, result);
    let queue = world.get_component_mut::<MclQueue>(request.target).unwrap();
    queue.commands.pop_front();
    queue.pending = None;
    queue.drain.completed(cleanup_error);
}

async fn execute_command(context: CommandContext) -> Result<CommandOutput, CommandError> {
    let request = context.request.clone();
    let outcome = AssertUnwindSafe(async {
        let operation = execute_operation(&context);
        tokio::pin!(operation);
        let result = tokio::select! {
            biased;
            () = request.cancel.cancelled() => { context.cancel.cancel(); operation.await },
            result = &mut operation => result,
        };
        let audit = storage(&context, StorageOperation::AppendHistory { records: vec![storage_core::HistoryRecord {
            source: request.source_address.clone(),
            turn: context.turn.clone(), kind: "mcl".into(), content: request.instruction.text.clone(),
            payload: json!({"id": request.id, "source_instance": request.source_instance, "text": request.instruction.text, "arguments": request.instruction.arguments, "succeeded": result.is_ok(), "error": result.as_ref().err()}), created_at_ms: None,
        }] }, CancellationToken::new()).await;
        match audit { Ok(_) => result, Err(audit) => Err(audit) }
    }).catch_unwind().await.unwrap_or_else(|_| Err(error(MclErrorKind::CleanupFailed, "MCL execution panicked; cleanup could not be confirmed")));
    match outcome {
        Ok(value) => Ok(CommandOutput { request, value }),
        Err(error) => Err(CommandError {
            request: Some(request),
            error,
        }),
    }
}

async fn execute_operation(context: &CommandContext) -> Result<MclValue, MclError> {
    if context.cancel.is_cancelled() || context.request.cancel.is_cancelled() {
        return Err(error(MclErrorKind::Cancelled, "MCL command cancelled"));
    }
    match context.operation.clone() {
        MclOperation::Create {
            name,
            fields,
            temporary,
        } => {
            let instance = if temporary {
                Some(context.instance.clone().ok_or_else(|| {
                    error(
                        MclErrorKind::SourceInvalid,
                        "TEMP block requires a current Driver VM",
                    )
                })?)
            } else {
                None
            };
            expect_unit(
                storage(
                    context,
                    StorageOperation::CreateBlock {
                        name,
                        fields,
                        instance,
                    },
                    context.cancel.clone(),
                )
                .await?,
            )
        }
        MclOperation::Get(expression) => Ok(MclValue::Array(MessageArray::Message(
            read_expression(context, expression).await?,
        ))),
        MclOperation::Inject { source, target } => {
            let sources = match source {
                MessageParameter::Empty => Vec::new(),
                MessageParameter::Manual(index) => vec![storage_core::MessageSource::Literal(
                    manual_messages(context, index)?,
                )],
                MessageParameter::Expression(expression) => {
                    prepare_sources(context, expression).await?
                }
            };
            expect_unit(
                storage(
                    context,
                    StorageOperation::ReplaceMessages {
                        sources,
                        target: selector(context, target),
                    },
                    context.cancel.clone(),
                )
                .await?,
            )
        }
        MclOperation::Config(operation) => {
            let operation = match operation {
                ConfigOperation::Get => StorageOperation::ReadConfig,
                ConfigOperation::Set { path, value } => StorageOperation::SetConfig { path, value },
                ConfigOperation::Delete { path } => StorageOperation::DeleteConfig { path },
            };
            match storage(context, operation, context.cancel.clone()).await? {
                StorageValue::Config(value) => Ok(MclValue::Config(value)),
                value => expect_unit(value),
            }
        }
        MclOperation::Import {
            kind,
            locator,
            alias,
        } => execute_import(context, kind, locator, alias).await,
        MclOperation::Effect { name, arguments } => execute_effect(context, &name, arguments).await,
    }
}

async fn storage(
    context: &CommandContext,
    operation: StorageOperation,
    cancel: CancellationToken,
) -> Result<StorageValue, MclError> {
    let (sender, receiver) = oneshot::channel();
    context.events.send_event(StorageRequest {
        owner: context.request.target,
        operation,
        cancel,
        reply: Arc::new(Mutex::new(Some(sender))),
    });
    receiver
        .await
        .map_err(|_| {
            error(
                MclErrorKind::CleanupFailed,
                "storage reply was lost before completion",
            )
        })?
        .map_err(storage_error)
}

fn storage_error(error_value: StorageError) -> MclError {
    let kind = match error_value.kind {
        StorageErrorKind::InvalidRange => MclErrorKind::InvalidRange,
        StorageErrorKind::ConfigPathInvalid => MclErrorKind::ConfigPathInvalid,
        StorageErrorKind::ConfigTypeMismatch => MclErrorKind::ConfigTypeMismatch,
        StorageErrorKind::Cancelled => MclErrorKind::Cancelled,
        StorageErrorKind::CommitUnknown => MclErrorKind::CommitUnknown,
        StorageErrorKind::CleanupFailed | StorageErrorKind::WorkerFailed => {
            MclErrorKind::CleanupFailed
        }
        _ => MclErrorKind::StorageFailed,
    };
    error(kind, error_value.to_string())
}

fn expect_unit(value: StorageValue) -> Result<MclValue, MclError> {
    if matches!(value, StorageValue::Unit) {
        Ok(MclValue::Unit)
    } else {
        Err(error(
            MclErrorKind::StorageFailed,
            "unexpected storage result",
        ))
    }
}

fn selector(context: &CommandContext, selector: Selector) -> storage_core::MessageSelector {
    storage_core::MessageSelector {
        block: selector.block,
        field: selector.field,
        instance: context.instance.clone(),
        selection: match selector.range {
            Selection::All => storage_core::MessageSelection::All,
            Selection::Index(index) => storage_core::MessageSelection::Index(index),
            Selection::Range(start, end) => storage_core::MessageSelection::Range(start, end),
        },
    }
}

async fn prepare_sources(
    context: &CommandContext,
    expression: MessageExpression,
) -> Result<Vec<storage_core::MessageSource>, MclError> {
    let mut sources = Vec::new();
    for source in expression.sources {
        match source {
            MessageSource::Selector(value) => sources.push(storage_core::MessageSource::Stored(
                selector(context, value),
            )),
            MessageSource::PromptAlias(alias) => {
                let path = context.prompts.get(&alias).cloned().ok_or_else(|| {
                    error(
                        MclErrorKind::AliasMissing,
                        format!("prompt alias {alias} is not imported"),
                    )
                })?;
                let cancel = context.cancel.clone();
                let prompt = tokio::task::spawn_blocking(move || {
                    if cancel.is_cancelled() {
                        return Err(error(MclErrorKind::Cancelled, "prompt read cancelled"));
                    }
                    read_prompt(&path, &PromptReadLimits::default())
                        .map_err(|failed| error(MclErrorKind::StorageFailed, failed.to_string()))
                })
                .await
                .map_err(|failed| error(MclErrorKind::CleanupFailed, failed.to_string()))??;
                let kind = match prompt.role {
                    PromptRole::System => "system",
                    PromptRole::User => "user",
                    PromptRole::Assistant => "assistant",
                };
                sources.push(storage_core::MessageSource::Literal(vec![
                    json!({"type": kind, "content": prompt.content}),
                ]));
            }
        }
    }
    Ok(sources)
}

async fn read_expression(
    context: &CommandContext,
    expression: MessageExpression,
) -> Result<Vec<Value>, MclError> {
    let sources = prepare_sources(context, expression).await?;
    match storage(
        context,
        StorageOperation::ReadMessageSources { sources },
        context.cancel.clone(),
    )
    .await?
    {
        StorageValue::Messages(messages) => {
            validate_messages(&messages)?;
            Ok(messages)
        }
        _ => Err(error(
            MclErrorKind::StorageFailed,
            "storage did not return messages",
        )),
    }
}

fn manual_messages(context: &CommandContext, index: usize) -> Result<Vec<Value>, MclError> {
    let messages = context
        .request
        .instruction
        .arguments
        .get(index)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            error(
                MclErrorKind::InvalidMessage,
                "message parameter must be a complete message array",
            )
        })?
        .clone();
    validate_messages(&messages)?;
    Ok(messages)
}

async fn effect_messages(
    context: &CommandContext,
    parameter: EffectParameter,
) -> Result<Vec<Value>, MclError> {
    match parameter {
        EffectParameter::Expression(expression) => read_expression(context, expression).await,
        EffectParameter::Manual(index) => manual_messages(context, index),
    }
}

fn alias_parameter(
    context: &CommandContext,
    parameter: EffectParameter,
) -> Result<Vec<String>, MclError> {
    let EffectParameter::Manual(index) = parameter else {
        return Err(error(
            MclErrorKind::ArgumentMismatch,
            "aliases must be passed as an argument array",
        ));
    };
    let values = context
        .request
        .instruction
        .arguments
        .get(index)
        .and_then(Value::as_array)
        .ok_or_else(|| error(MclErrorKind::ArgumentMismatch, "aliases must be an array"))?;
    let mut unique = BTreeSet::new();
    let mut aliases = Vec::new();
    for value in values {
        let alias = value
            .as_str()
            .filter(|alias| !alias.trim().is_empty())
            .ok_or_else(|| {
                error(
                    MclErrorKind::ArgumentMismatch,
                    "alias must be a nonempty string",
                )
            })?;
        if !unique.insert(alias) {
            return Err(error(
                MclErrorKind::AliasConflict,
                "duplicate alias in effect argument",
            ));
        }
        aliases.push(alias.to_owned());
    }
    Ok(aliases)
}

async fn in_world<T: Send + 'static>(
    sender: &WorldClosureSender,
    work: impl FnOnce(&mut World) -> Result<T, MclError> + Send + 'static,
) -> Result<T, MclError> {
    let (reply, receiver) = oneshot::channel();
    sender.send(move |world| {
        let _ = reply.send(work(world));
    });
    receiver
        .await
        .map_err(|_| error(MclErrorKind::CleanupFailed, "World action reply was lost"))?
}

fn check_action(
    world: &World,
    request: &MclCommand,
    cancel: &CancellationToken,
) -> Result<(), MclError> {
    if cancel.is_cancelled() {
        return Err(error(
            MclErrorKind::Cancelled,
            "MCL command cancelled before World action",
        ));
    }
    validate_source(world, request)?;
    let queue = world
        .get_component::<MclQueue>(request.target)
        .ok_or_else(|| error(MclErrorKind::Closed, "MCL queue is missing"))?;
    if !queue.accepting
        || queue.cancel.is_cancelled()
        || queue.pending.as_deref() != Some(request.id.as_str())
        || !queue
            .commands
            .front()
            .is_some_and(|front| Arc::ptr_eq(&front.reply, &request.reply))
    {
        return Err(error(
            MclErrorKind::Closed,
            "MCL queue no longer owns this command",
        ));
    }
    Ok(())
}

fn alias_exists(world: &World, owner: Entity, alias: &str) -> bool {
    world
        .get_component::<Prompts>(owner)
        .and_then(|entries| entries.get(alias))
        .is_some()
        || world
            .get_component::<Tools>(owner)
            .and_then(|entries| entries.get(alias))
            .is_some()
        || world
            .get_component::<Sandboxes>(owner)
            .and_then(|entries| entries.get(alias))
            .is_some()
}

async fn execute_import(
    context: &CommandContext,
    kind: ImportKind,
    locator: String,
    alias: String,
) -> Result<MclValue, MclError> {
    let parsed = ParsedAddress::parse(&locator)
        .map_err(|failed| error(MclErrorKind::ImportDenied, failed.to_string()))?;
    let expected = match kind {
        ImportKind::Message => "prompt",
        ImportKind::Tool => "tool",
        ImportKind::Sandbox => "sandbox",
    };
    if parsed.kind != expected
        || !context
            .allowed_imports
            .iter()
            .filter_map(|value| ParsedAddress::parse(value).ok())
            .any(|value| value == parsed)
    {
        return Err(error(
            MclErrorKind::ImportDenied,
            "resource kind or Driver dependency does not permit import",
        ));
    }
    let root = match kind {
        ImportKind::Message => &context.imports.prompt_root,
        ImportKind::Tool => &context.imports.tool_root,
        ImportKind::Sandbox => &context.imports.sandbox_root,
    }
    .clone();
    let cancel = context.cancel.clone();
    let path = tokio::task::spawn_blocking(move || {
        if cancel.is_cancelled() {
            return Err(error(
                MclErrorKind::Cancelled,
                "resource resolution cancelled",
            ));
        }
        resolve_file(&root, &locator)
            .map_err(|failed| error(MclErrorKind::ImportDenied, failed.to_string()))
    })
    .await
    .map_err(|failed| error(MclErrorKind::CleanupFailed, failed.to_string()))??;
    let request = context.request.clone();
    let cancel = context.cancel.clone();
    let response = in_world(&context.world, move |world| {
        check_action(world, &request, &cancel)?;
        if alias_exists(world, request.target, &alias) {
            return Err(error(
                MclErrorKind::AliasConflict,
                "alias is already registered in this Agent",
            ));
        }
        let events = world.event_sender();
        Ok((events, request.target, alias, path, cancel))
    })
    .await?;
    let (events, owner, alias, path, cancel) = response;
    let result = match kind {
        ImportKind::Message => {
            let (sender, receiver) = oneshot::channel();
            events.send_event(ImportPrompt {
                owner,
                alias: alias.clone(),
                path,
                cancel,
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            receiver
                .await
                .map_err(|_| error(MclErrorKind::CleanupFailed, "prompt import reply was lost"))?
                .map_err(|failed| error(MclErrorKind::ImportDenied, failed.to_string()))
        }
        ImportKind::Tool => {
            let (sender, receiver) = oneshot::channel();
            events.send_event(ImportTool {
                owner,
                alias: alias.clone(),
                path,
                cancel,
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            receiver
                .await
                .map_err(|_| error(MclErrorKind::CleanupFailed, "tool import reply was lost"))?
                .map_err(|failed| error(MclErrorKind::ImportDenied, failed.to_string()))
        }
        ImportKind::Sandbox => {
            let (sender, receiver) = oneshot::channel();
            events.send_event(ImportSandbox {
                owner,
                alias: alias.clone(),
                path,
                cancel,
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            receiver
                .await
                .map_err(|_| error(MclErrorKind::CleanupFailed, "sandbox import reply was lost"))?
                .map_err(|failed| error(MclErrorKind::ImportDenied, failed.to_string()))
        }
    };
    result?;
    Ok(MclValue::Import { kind, alias })
}

async fn execute_effect(
    context: &CommandContext,
    name: &str,
    arguments: Vec<EffectParameter>,
) -> Result<MclValue, MclError> {
    let mut arguments = arguments.into_iter();
    match name {
        "inference" => {
            let messages = effect_messages(context, arguments.next().unwrap()).await?;
            let tools = alias_parameter(context, arguments.next().unwrap())?;
            execute_inference(context, messages, tools).await
        }
        "tool_call" => {
            let messages = effect_messages(context, arguments.next().unwrap()).await?;
            let tools = alias_parameter(context, arguments.next().unwrap())?;
            execute_tools(context, messages, tools).await
        }
        "history_append" => {
            let messages = effect_messages(context, arguments.next().unwrap()).await?;
            let records = messages
                .into_iter()
                .map(|message| storage_core::HistoryRecord {
                    source: context.request.source_address.clone(),
                    turn: context.turn.clone(),
                    kind: "message".into(),
                    content: message
                        .get("content")
                        .or_else(|| message.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    payload: message,
                    created_at_ms: None,
                })
                .collect();
            expect_unit(
                storage(
                    context,
                    StorageOperation::AppendHistory { records },
                    context.cancel.clone(),
                )
                .await?,
            )
        }
        "sandbox_use" => {
            let mut aliases = alias_parameter(context, arguments.next().unwrap())?;
            if aliases.len() > 1 {
                return Err(error(
                    MclErrorKind::ArgumentMismatch,
                    "sandbox_use expects zero or one alias",
                ));
            }
            let alias = aliases.pop();
            let request = context.request.clone();
            let cancel = context.cancel.clone();
            in_world(&context.world, move |world| {
                check_action(world, &request, &cancel)?;
                world
                    .sandbox_select(request.target, alias)
                    .map_err(|failed| error(MclErrorKind::AliasMissing, failed.to_string()))?;
                Ok(MclValue::Unit)
            })
            .await
        }
        "start_turn" => {
            let request = context.request.clone();
            let cancel = context.cancel.clone();
            in_world(&context.world, move |world| {
                check_action(world, &request, &cancel)?;
                world
                    .agent_start_turn(request.target)
                    .map_err(|failed| error(MclErrorKind::EffectFailed, failed.to_string()))?;
                Ok(MclValue::Unit)
            })
            .await
        }
        "finish_turn" => finish_turn(context).await,
        "ready" | "wake" => {
            let ready = name == "ready";
            let request = context.request.clone();
            let cancel = context.cancel.clone();
            in_world(&context.world, move |world| {
                check_action(world, &request, &cancel)?;
                let vm = world
                    .get_component::<DriverLuaVm>(request.target)
                    .ok_or_else(|| error(MclErrorKind::SourceInvalid, "target VM is missing"))?;
                let instance = vm.instance.clone();
                if ready
                    && (request.source != request.target
                        || request.source_instance.as_deref() != Some(instance.start_id.as_str())
                        || world.get_component::<Agent>(request.target).unwrap().state
                            != AgentState::Starting
                            && world.get_component::<Agent>(request.target).unwrap().state
                                != AgentState::Restarting)
                {
                    return Err(error(
                        MclErrorKind::SourceInvalid,
                        "ready is only allowed for the starting Driver on itself",
                    ));
                }
                if ready {
                    world.driver_ready(&instance)
                } else {
                    world.driver_wake(&instance)
                }
                .map_err(|failed| error(MclErrorKind::EffectFailed, failed.to_string()))?;
                Ok(MclValue::Unit)
            })
            .await
        }
        _ => Err(error(MclErrorKind::ParseFailed, "unsupported effect")),
    }
}

async fn finish_turn(context: &CommandContext) -> Result<MclValue, MclError> {
    loop {
        let request = context.request.clone();
        let cancel = context.cancel.clone();
        let expected_turn = context.turn.clone();
        let finished = in_world(&context.world, move |world| {
            check_action(world, &request, &cancel)?;
            let agent = world.get_component::<Agent>(request.target).unwrap();
            if agent.state != AgentState::Working
                || agent
                    .turn
                    .as_ref()
                    .is_some_and(|turn| Some(turn) != expected_turn.as_ref())
            {
                return Err(error(
                    MclErrorKind::EffectFailed,
                    "turn changed before finish_turn",
                ));
            }
            if world
                .get_component::<ActiveInference>(request.target)
                .and_then(|active| active.current.as_ref())
                .is_some_and(|handle| !handle.is_finished())
            {
                return Ok(false);
            }
            if let Some(active) = world.get_component_mut::<ActiveInference>(request.target) {
                active.current.take();
            }
            world
                .agent_finish_turn(
                    request.target,
                    expected_turn.as_deref().unwrap_or(request.id.as_str()),
                )
                .map_err(|failed| error(MclErrorKind::EffectFailed, failed.to_string()))?;
            Ok(true)
        })
        .await?;
        if finished {
            return Ok(MclValue::Unit);
        }
    }
}

async fn execute_inference(
    context: &CommandContext,
    messages: Vec<Value>,
    tools: Vec<String>,
) -> Result<MclValue, MclError> {
    let mut typed = Vec::new();
    for message in messages {
        if message.get("type").and_then(Value::as_str) == Some("assistant")
            && message.get("stop_reason").and_then(Value::as_str) == Some("cancelled")
        {
            continue;
        }
        typed.push(inference_message(message)?);
    }
    let request = context.request.clone();
    let cancel = context.cancel.clone();
    let expected_turn = context.turn.clone();
    let receiver = in_world(&context.world, move |world| {
        check_action(world, &request, &cancel)?;
        let agent = world.get_component::<Agent>(request.target).unwrap();
        if agent.state != AgentState::Working || agent.turn.is_none() || agent.turn != expected_turn
        {
            return Ok(None);
        }
        for alias in &tools {
            if world
                .get_component::<Tools>(request.target)
                .and_then(|entries| entries.get(alias))
                .is_none()
            {
                return Err(error(
                    MclErrorKind::AliasMissing,
                    format!("tool alias {alias} is not imported"),
                ));
            }
        }
        let config = world
            .get_component::<AgentInferenceConfig>(request.target)
            .ok_or_else(|| {
                error(
                    MclErrorKind::EffectFailed,
                    "Agent inference configuration is missing",
                )
            })?
            .parameters
            .clone();
        let (sender, receiver) = oneshot::channel();
        world.send_event(InferenceRequest {
            id: request.id,
            agent: request.target,
            turn: agent.turn.clone().unwrap(),
            messages: typed,
            tools,
            config,
            cancel,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        Ok(Some(receiver))
    })
    .await?;
    let response = match receiver {
        None => InferenceResponse::cancelled(),
        Some(receiver) => match receiver
            .await
            .map_err(|_| error(MclErrorKind::CleanupFailed, "inference reply was lost"))?
        {
            Ok(response) => response,
            Err(failed) => {
                return Ok(MclValue::Inference {
                    message: json!({
                        "type": "error",
                        "message": failed.to_string(),
                        "request_id": context.request.id,
                    }),
                    stop_reason: "error".into(),
                });
            }
        },
    };
    inference_result(response, &context.request.id)
}

fn inference_result(response: InferenceResponse, request_id: &str) -> Result<MclValue, MclError> {
    let stop_reason = response.stop_reason.as_str().to_owned();
    let calls = response.tool_calls.into_iter().map(|call| {
        Ok(json!({"id": call.id, "tool_name": call.name, "arguments": serde_json::to_string(&call.arguments).map_err(|failed| error(MclErrorKind::EffectFailed, failed.to_string()))?}))
    }).collect::<Result<Vec<_>, MclError>>()?;
    let mut message = json!({"request_id": request_id, "type": "assistant", "reasoning": response.reasoning, "content": response.content, "tool_calls": calls, "replay": response.replay, "usage": response.usage, "stop_reason": stop_reason});
    message
        .as_object_mut()
        .unwrap()
        .retain(|_, value| !value.is_null());
    Ok(MclValue::Inference {
        message,
        stop_reason,
    })
}

async fn execute_tools(
    context: &CommandContext,
    messages: Vec<Value>,
    tools: Vec<String>,
) -> Result<MclValue, MclError> {
    let [message] = messages.as_slice() else {
        return Err(error(
            MclErrorKind::InvalidMessage,
            "tool_call expects exactly one Assistant message",
        ));
    };
    if message.get("type").and_then(Value::as_str) != Some("assistant") {
        return Err(error(
            MclErrorKind::InvalidMessage,
            "tool_call expects an Assistant message",
        ));
    }
    let calls = assistant_calls(message)?;
    let request = context.request.clone();
    let cancel = context.cancel.clone();
    let expected_turn = context.turn.clone();
    static NEXT_TOOL_BATCH: AtomicU64 = AtomicU64::new(1);
    let task_prefix = format!(
        "{}:{}",
        std::process::id(),
        NEXT_TOOL_BATCH.fetch_add(1, Ordering::Relaxed)
    );
    let pending = in_world(&context.world, move |world| {
        check_action(world, &request, &cancel)?;
        let agent = world.get_component::<Agent>(request.target).unwrap();
        let active_turn = agent.state == AgentState::Working
            && agent.turn.is_some()
            && agent.turn == expected_turn;
        let visible = tools.into_iter().collect::<BTreeSet<_>>();
        let mut pending = Vec::new();
        for (index, call) in calls.into_iter().enumerate() {
            if !active_turn {
                pending.push((call, None, Some("Tool call aborted.".to_owned())));
                continue;
            }
            if !visible.contains(&call.name)
                || world
                    .get_component::<Tools>(request.target)
                    .and_then(|entries| entries.get(&call.name))
                    .is_none()
            {
                pending.push((call, None, Some("tool not visible".to_owned())));
                continue;
            }
            let (sender, receiver) = oneshot::channel();
            world.send_event(ToolExecuteRequest {
                id: format!("tool:{task_prefix}:{index}"),
                agent: request.target,
                alias: call.name.clone(),
                call_id: call.id.clone(),
                arguments: call.arguments.clone(),
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            pending.push((call, Some(receiver), None));
        }
        Ok(pending)
    })
    .await?;
    let mut results = Vec::new();
    let mut failure = None;
    for (call, receiver, fallback) in pending {
        if let Some(content) = fallback {
            results.push(tool_message(&call, content, Vec::new(), true));
            continue;
        }
        match receiver.unwrap().await {
            Ok(Ok(response)) => {
                if (!response.call_id.is_empty() && response.call_id != call.id)
                    || (!response.alias.is_empty() && response.alias != call.name)
                {
                    failure.get_or_insert_with(|| {
                        error(
                            MclErrorKind::EffectFailed,
                            "tool response association does not match request",
                        )
                    });
                } else {
                    results.push(tool_message(
                        &call,
                        response.content,
                        response.images,
                        response.failed,
                    ));
                }
            }
            Ok(Err(failed)) => {
                let kind = if matches!(
                    failed.kind,
                    ToolErrorKind::CleanupFailed | ToolErrorKind::TaskPanicked
                ) {
                    MclErrorKind::CleanupFailed
                } else {
                    MclErrorKind::EffectFailed
                };
                failure.get_or_insert_with(|| error(kind, failed.to_string()));
            }
            Err(_) => {
                failure.get_or_insert_with(|| {
                    error(MclErrorKind::CleanupFailed, "tool call reply was lost")
                });
            }
        }
    }
    if let Some(error) = failure {
        Err(error)
    } else {
        Ok(MclValue::Array(MessageArray::Message(results)))
    }
}

fn tool_message(
    call: &InferenceToolCall,
    content: String,
    images: Vec<tool_core::ToolImage>,
    failed: bool,
) -> Value {
    json!({"type": "tool_call", "tool_call_id": call.id, "tool_name": call.name, "content": content, "images": images, "failed": failed})
}

fn assistant_calls(message: &Value) -> Result<Vec<InferenceToolCall>, MclError> {
    let calls = match message.get("tool_calls") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(calls)) => calls,
        _ => {
            return Err(error(
                MclErrorKind::InvalidMessage,
                "Assistant tool_calls must be an array",
            ));
        }
    };
    let mut unique = BTreeSet::new();
    calls
        .iter()
        .map(|call| {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| error(MclErrorKind::InvalidMessage, "tool call id is missing"))?;
            if !unique.insert(id) {
                return Err(error(
                    MclErrorKind::InvalidMessage,
                    "duplicate tool call id",
                ));
            }
            let name = call
                .get("tool_name")
                .or_else(|| call.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| error(MclErrorKind::InvalidMessage, "tool call name is missing"))?;
            let arguments = call.get("arguments").ok_or_else(|| {
                error(
                    MclErrorKind::InvalidMessage,
                    "tool call arguments are missing",
                )
            })?;
            let arguments = match arguments {
                Value::String(arguments) => serde_json::from_str(arguments).map_err(|_| {
                    error(
                        MclErrorKind::InvalidMessage,
                        "tool call arguments are not JSON",
                    )
                })?,
                value => value.clone(),
            };
            Ok(InferenceToolCall {
                id: id.into(),
                name: name.into(),
                arguments,
            })
        })
        .collect()
}

fn inference_message(mut message: Value) -> Result<InferenceMessage, MclError> {
    let kind = message
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| error(MclErrorKind::InvalidMessage, "message type is missing"))?
        .to_owned();
    if kind == "assistant" {
        let calls = assistant_calls(&message)?;
        message.as_object_mut().unwrap().insert(
            "tool_calls".into(),
            serde_json::to_value(calls)
                .map_err(|failed| error(MclErrorKind::InvalidMessage, failed.to_string()))?,
        );
    } else if kind == "tool_call" {
        let call_id = message
            .get("tool_call_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                error(
                    MclErrorKind::InvalidMessage,
                    "tool message call id is missing",
                )
            })?
            .to_owned();
        let object = message.as_object_mut().unwrap();
        object.insert("type".into(), json!("tool"));
        object.insert("call_id".into(), json!(call_id));
    }
    serde_json::from_value(message)
        .map_err(|failed| error(MclErrorKind::InvalidMessage, failed.to_string()))
}

fn validate_messages(messages: &[Value]) -> Result<(), MclError> {
    for message in messages {
        if message.get("type").and_then(Value::as_str) == Some("inject") {
            let nested = message
                .get("messages")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    error(
                        MclErrorKind::InvalidMessage,
                        "inject message must contain a message array",
                    )
                })?;
            validate_messages(nested)?;
        } else if message.get("type").and_then(Value::as_str) == Some("error") {
            if message.get("message").and_then(Value::as_str).is_none() {
                return Err(error(
                    MclErrorKind::InvalidMessage,
                    "error message must contain a message string",
                ));
            }
        } else {
            inference_message(message.clone())?;
        }
    }
    Ok(())
}
