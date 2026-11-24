use crate::types::*;
use crate::{InstalledMcl, MclImportConfig};
use address::{Address, ParsedAddress, resolve_file};
use agent_core::{Agent, AgentState, WorldAgentExt};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::{Entity, World};
use futures_util::FutureExt;
use inference_core::{
    ActiveInference, AgentInferenceConfig, InferenceMessage, InferenceRequest, InferenceResponse,
    InferenceToolCall,
};
use lua_runtime_core::DriverLuaVm;
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
use tool_core::ToolExecuteRequest;
use tool_resource_core::{ImportTool, Tools};

fn error(kind: MclErrorKind, message: impl Into<String>) -> MclError {
    MclError::new(kind, message)
}

pub(crate) fn respond(request: &MclCommand, result: Result<MclValue, MclError>) {
    if let Some(sender) = lock(&request.reply).take() {
        let result = result.unwrap_or_else(|failed| {
            MclValue::Error(json!({
                "type": "error",
                "message": failed.to_string(),
                "request_id": request.id,
            }))
        });
        let _ = sender.send(Ok(result));
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
    if !matches!(agent.state(), AgentState::Starting | AgentState::Running) {
        return Err(error(
            MclErrorKind::Closed,
            "Agent is not accepting commands",
        ));
    }
    if let Some(instance) = &request.source_instance {
        if instance.starts_with("task-instance:") {
            let tasks = world
                .get_component::<tool_core::ToolTasks>(request.source)
                .ok_or_else(|| error(MclErrorKind::SourceInvalid, "source tasks missing"))?;
            let handle = tasks
                .list()
                .into_iter()
                .map(|(_, handle)| handle)
                .find(|handle| handle.execution_id() == instance)
                .ok_or_else(|| error(MclErrorKind::SourceInvalid, "source execution is stale"))?;
            if handle.cancel_token().is_cancelled() || handle.is_cleanup_complete() {
                return Err(error(MclErrorKind::SourceInvalid, "source task closed"));
            }
            return Ok(());
        }
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
        if queue.commands.iter().any(|queued| queued.id == request.id)
            || queue.active.lock().unwrap().contains_key(&request.id)
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
    loop {
        let Some(queue) = world.get_component_mut::<MclQueue>(target) else {
            return;
        };
        if queue.locked.load(Ordering::Acquire) {
            return;
        }
        let Some(request) = queue.commands.pop_front() else {
            return;
        };
        let rejected =
            if !queue.accepting || queue.cancel.is_cancelled() || request.cancel.is_cancelled() {
                Some(error(
                    MclErrorKind::Cancelled,
                    "MCL command cancelled before execution",
                ))
            } else {
                None
            };
        if let Some(failed) = rejected {
            respond(&request, Err(failed));
            queue.drain.completed(None);
            continue;
        }
        if let Err(failed) = validate_source(world, &request) {
            respond(&request, Err(failed));
            world
                .get_component::<MclQueue>(target)
                .unwrap()
                .drain
                .completed(None);
            continue;
        }
        let operation = match parse_instruction(&request.instruction) {
            Ok(operation) => operation,
            Err(failed) => {
                respond(&request, Err(failed));
                world
                    .get_component::<MclQueue>(target)
                    .unwrap()
                    .drain
                    .completed(None);
                continue;
            }
        };
        let owns_lock = !matches!(&operation,MclOperation::Effect{name,..}if matches!(name.as_str(),"tool_call"|"inference"|"finish_turn"));
        let schedule = world
            .get_resource::<InstalledMcl>()
            .expect("MCL plugin is not installed")
            .schedule
            .clone();
        let queue = world.get_component_mut::<MclQueue>(target).unwrap();
        if owns_lock {
            queue.locked.store(true, Ordering::Release);
        }
        queue
            .active
            .lock()
            .unwrap()
            .insert(request.id.clone(), request.reply.clone());
        let cancel = queue.cancel.child_token();
        let completion = CommandCompletion {
            request: request.clone(),
            active: queue.active.clone(),
            locked: queue.locked.clone(),
            owns_lock,
            drain: queue.drain.clone(),
            world: world.closure_sender(&schedule),
            settled: false,
        };
        let instance = world
            .get_component::<agent_core::Agent>(target)
            .map(|agent| agent.instance().to_owned());
        let context = CommandContext {
            request,
            operation,
            cancel,
            events: world.event_sender(),
            world: world.closure_sender(&schedule),
            instance,
            turn: world
                .get_component::<Agent>(target)
                .and_then(|agent| agent.turn().clone()),
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
            completion,
        };
        world
            .get_resource::<AsyncRuntimeHandle>()
            .expect("MCL async runtime missing")
            .spawn(execute_command(context));
        if owns_lock {
            return;
        }
    }
}

async fn execute_command(mut context: CommandContext) {
    let request = context.request.clone();
    let outcome = AssertUnwindSafe(async {
        let operation = execute_operation(&context);
        tokio::pin!(operation);
        let result = tokio::select! {
            biased;
            () = request.cancel.cancelled() => { context.cancel.cancel(); operation.await },
            result = &mut operation => result,
        };
        let audit_turn=match &context.operation{MclOperation::Effect{name,arguments}if matches!(name.as_str(),"inference"|"tool_call")=>explicit_turn(&context,arguments.get(1).cloned())?,_=>context.turn.clone()};
        let audit = storage(&context, StorageOperation::AppendHistory { records: vec![storage_core::HistoryRecord {
            source: Some(request.source_address.clone()),
            turn: audit_turn, kind: "mcl".into(), content: request.instruction.text.clone(),
            payload: json!({"id": request.id, "source_instance": request.source_instance, "text": request.instruction.text, "arguments": request.instruction.arguments, "succeeded": result.is_ok(), "error": result.as_ref().err()}), created_at_ms: None,
        }] }, CancellationToken::new()).await;
        match audit {
            Ok(_) => result,
            Err(audit) => Err(error(audit.kind, format!(
                "MCL audit persistence failed after execution; side effects may already have occurred; execution outcome: {}; audit error: {audit}",
                match &result {
                    Ok(_) => "succeeded".to_owned(),
                    Err(failed) => format!("failed ({failed})"),
                }
            ))),
        }
    }).catch_unwind().await.unwrap_or_else(|_| Err(error(MclErrorKind::CleanupFailed, "MCL execution panicked; cleanup could not be confirmed")));
    context.completion.finish(outcome);
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
        EffectParameter::ToolCalls(_) => Err(error(
            MclErrorKind::InvalidMessage,
            "tool_calls is not a message expression",
        )),
        EffectParameter::Manual(index) => manual_messages(context, index),
    }
}

// Tool execution validates identities and arguments per call in executable_calls.
// Reading and alias failures remain ordinary MCL errors at this stage.
async fn tool_effect_messages(
    context: &CommandContext,
    parameter: EffectParameter,
) -> Result<Vec<Value>, MclError> {
    match parameter {
        EffectParameter::Manual(index) => context
            .request
            .instruction
            .arguments
            .get(index)
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                error(
                    MclErrorKind::InvalidMessage,
                    "message parameter must be a complete message array",
                )
            }),
        EffectParameter::ToolCalls(expression) => {
            let sources = prepare_sources(context, expression).await?;
            let messages = match storage(
                context,
                StorageOperation::ReadMessageSources { sources },
                context.cancel.clone(),
            )
            .await?
            {
                StorageValue::Messages(messages) => messages,
                _ => {
                    return Err(error(
                        MclErrorKind::StorageFailed,
                        "storage returned no messages",
                    ));
                }
            };
            let [message] = messages.as_slice() else {
                return Err(error(
                    MclErrorKind::InvalidMessage,
                    "tool_calls projection requires one message",
                ));
            };
            message
                .get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| {
                    error(
                        MclErrorKind::InvalidMessage,
                        "selected message tool_calls missing",
                    )
                })
        }
        EffectParameter::Expression(expression) => {
            let sources = prepare_sources(context, expression).await?;
            match storage(
                context,
                StorageOperation::ReadMessageSources { sources },
                context.cancel.clone(),
            )
            .await?
            {
                StorageValue::Messages(messages) => Ok(messages),
                _ => Err(error(
                    MclErrorKind::StorageFailed,
                    "storage did not return messages",
                )),
            }
        }
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
        || !queue
            .active
            .lock()
            .unwrap()
            .get(&request.id)
            .is_some_and(|reply| Arc::ptr_eq(reply, &request.reply))
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
    if parsed.kind != expected {
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
        "model_set" => {
            let parameter = arguments.next().unwrap();
            let EffectParameter::Manual(index) = parameter else {
                return Err(error(
                    MclErrorKind::ArgumentMismatch,
                    "model_set requires ?",
                ));
            };
            let parameters: inference_core::InferenceParameters =
                serde_json::from_value(context.request.instruction.arguments[index].clone())
                    .map_err(|e| error(MclErrorKind::InvalidMessage, e.to_string()))?;
            parameters
                .validate()
                .map_err(|e| error(MclErrorKind::InvalidMessage, e.to_string()))?;
            let request = context.request.clone();
            let cancel = context.cancel.clone();
            in_world(&context.world, move |world| {
                check_action(world, &request, &cancel)?;
                let routes = world
                    .get_resource::<inference_core::ModelRoutes>()
                    .ok_or_else(|| error(MclErrorKind::EffectFailed, "ModelRoutes missing"))?
                    .clone();
                let route=routes.get(&parameters.model).ok_or_else(||error(MclErrorKind::EffectFailed,"model route missing"))?.clone();
                let info=json!({"id":parameters.model,"model":route.model,"provider":route.provider,"api_type":route.api_type,"context_window_tokens":route.context_window_tokens});
                if let Some(config) =
                    world.get_component_mut::<inference_core::AgentInferenceConfig>(request.target)
                {
                    config.set(parameters, &routes);
                } else {
                    let binding = world
                        .get_resource::<inference_core::ModelObserverFactory>()
                        .ok_or_else(|| error(MclErrorKind::EffectFailed, "model binding missing"))?
                        .for_owner(request.target);
                    let config = inference_core::AgentInferenceConfig::new(parameters, binding);
                    config.publish_current(&routes);
                    world.insert_component(request.target, config);
                }
                let value=toml::Value::try_from(info).map_err(|e|error(MclErrorKind::EffectFailed,e.to_string()))?;
                Ok(MclValue::Config(value.as_table().unwrap().clone()))
            })
            .await
        }
        "inference" => {
            let messages = effect_messages(context, arguments.next().unwrap()).await?;
            let turn = explicit_turn(context, arguments.next())?;
            execute_inference(context, messages, turn).await
        }
        "tool_call" => {
            let messages = tool_effect_messages(context, arguments.next().unwrap()).await?;
            let turn = explicit_turn(context, arguments.next())?;
            execute_tools(context, messages, turn).await
        }
        "block_snapshot" | "block_snapshots" | "block_restore" => {
            execute_block_effect(context, name, arguments.collect()).await
        }
        "history_append" => {
            let messages = effect_messages(context, arguments.next().unwrap()).await?;
            let request = context.request.clone();
            let cancel = context.cancel.clone();
            let storage = in_world(&context.world, move |world| {
                check_action(world, &request, &cancel)?;
                world
                    .get_component::<storage_core::AgentStorage>(request.target)
                    .cloned()
                    .ok_or_else(|| error(MclErrorKind::StorageFailed, "Agent storage is missing"))
            })
            .await?;
            let turn = context.turn.clone();
            let cancel = context.cancel.clone();
            let recorded = tokio::task::spawn_blocking(move || {
                if cancel.is_cancelled() {
                    return Err(error(
                        MclErrorKind::Cancelled,
                        "history append cancelled before execution",
                    ));
                }
                storage_core::append_message_history(&storage, &messages, turn.as_deref())
                    .map_err(storage_error)
            })
            .await
            .map_err(|failed| error(MclErrorKind::CleanupFailed, failed.to_string()))??;
            Ok(MclValue::Array(MessageArray::Message(recorded)))
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
                if let Some(name) = alias.as_deref() {
                    world
                        .sandbox_path(request.target, name)
                        .map_err(|failed| error(MclErrorKind::AliasMissing, failed.to_string()))?;
                }
                if !world.contains_component::<sandbox_resource_core::Sandboxes>(request.target)
                    || !world.contains_component::<sandbox_resource_core::SelectedSandbox>(
                        request.target,
                    )
                {
                    let (entries_changed, selection_changed) = world
                        .get_resource::<sandbox_resource_core::SandboxObserverFactory>()
                        .ok_or_else(|| {
                            error(
                                MclErrorKind::EffectFailed,
                                "sandbox observer factory missing",
                            )
                        })?
                        .for_owner(request.target);
                    if !world.contains_component::<sandbox_resource_core::Sandboxes>(request.target)
                    {
                        world.insert_component(
                            request.target,
                            sandbox_resource_core::Sandboxes::new(entries_changed),
                        );
                    }
                    if !world.contains_component::<sandbox_resource_core::SelectedSandbox>(
                        request.target,
                    ) {
                        world.insert_component(
                            request.target,
                            sandbox_resource_core::SelectedSandbox::new(None, selection_changed),
                        );
                    }
                }
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
                    .map(MclValue::Turn)
                    .map_err(|failed| error(MclErrorKind::EffectFailed, failed.to_string()))
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
                if ready {
                    world.agent_ready(request.target).map_err(|error| {
                        self::error(MclErrorKind::EffectFailed, error.to_string())
                    })?;
                } else {
                    let slot = world
                        .get_component::<agent_core::Agent>(request.target)
                        .ok_or_else(|| {
                            error(MclErrorKind::TargetMissing, "Agent wait slot missing")
                        })?;
                    slot.wake_slot()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .wake()
                        .map_err(|e| error(MclErrorKind::EffectFailed, e.to_string()))?;
                }
                Ok(MclValue::Unit)
            })
            .await
        }
        _ => Err(error(MclErrorKind::ParseFailed, "unsupported effect")),
    }
}

fn block_effect_argument<'a>(
    context: &'a CommandContext,
    parameter: &EffectParameter,
) -> Result<&'a Value, MclError> {
    let EffectParameter::Manual(index) = parameter else {
        return Err(error(
            MclErrorKind::ArgumentMismatch,
            "block parameter requires ?",
        ));
    };
    context
        .request
        .instruction
        .arguments
        .get(*index)
        .ok_or_else(|| error(MclErrorKind::ArgumentMismatch, "missing block parameter"))
}

async fn execute_block_effect(
    context: &CommandContext,
    name: &str,
    parameters: Vec<EffectParameter>,
) -> Result<MclValue, MclError> {
    let block = block_effect_argument(context, &parameters[0])?
        .as_str()
        .ok_or_else(|| {
            error(
                MclErrorKind::ArgumentMismatch,
                "block name must be a logical identifier",
            )
        })?
        .to_owned();
    validate_identifier(&block).map_err(|_| {
        error(
            MclErrorKind::ArgumentMismatch,
            "block name must be a logical identifier",
        )
    })?;
    let snapshot_id = if name == "block_restore" {
        Some(
            block_effect_argument(context, &parameters[1])?
                .as_u64()
                .filter(|id| *id > 0 && *id <= i64::MAX as u64)
                .ok_or_else(|| {
                    error(
                        MclErrorKind::ArgumentMismatch,
                        "snapshot id must be a positive SQLite integer",
                    )
                })?,
        )
    } else {
        None
    };
    let request = context.request.clone();
    let cancel = context.cancel.clone();
    let storage = in_world(&context.world, move |world| {
        check_action(world, &request, &cancel)?;
        world
            .get_component::<storage_core::AgentStorage>(request.target)
            .cloned()
            .ok_or_else(|| error(MclErrorKind::StorageFailed, "Agent storage is missing"))
    })
    .await?;
    let instance = context.instance.clone();
    let name = name.to_owned();
    let cancel = context.cancel.clone();
    tokio::task::spawn_blocking(move || {
        if cancel.is_cancelled() {
            return Err(error(
                MclErrorKind::Cancelled,
                "block operation cancelled before execution",
            ));
        }
        match name.as_str() {
            "block_snapshot" => storage_core::snapshot_block(&storage, &block, instance.as_deref())
                .map(|()| MclValue::Unit)
                .map_err(storage_error),
            "block_snapshots" => {
                storage_core::read_block_snapshots(&storage, &block, instance.as_deref())
                    .map(MclValue::BlockSnapshots)
                    .map_err(storage_error)
            }
            "block_restore" => storage_core::restore_block(
                &storage,
                &block,
                instance.as_deref(),
                snapshot_id.unwrap(),
            )
            .map(|()| MclValue::Unit)
            .map_err(storage_error),
            _ => unreachable!("block effect dispatch"),
        }
    })
    .await
    .map_err(|failed| error(MclErrorKind::CleanupFailed, failed.to_string()))?
}

async fn finish_turn(context: &CommandContext) -> Result<MclValue, MclError> {
    let request = context.request.clone();
    let cancel = context.cancel.clone();
    let expected = context.turn.clone();
    let handle = in_world(&context.world, move |world| {
        check_action(world, &request, &cancel)?;
        Ok(world
            .get_component::<ActiveInference>(request.target)
            .and_then(|active| active.current.clone())
            .filter(|handle| expected.is_some() && handle.turn == expected))
    })
    .await?;
    let completed_request = handle.as_ref().map(|handle| handle.request_id.clone());
    if let Some(handle) = handle {
        handle
            .wait_finished()
            .await
            .map_err(|failure| error(MclErrorKind::CleanupFailed, failure.to_string()))?;
    }
    let request = context.request.clone();
    let cancel = context.cancel.clone();
    let expected = context.turn.clone();
    in_world(&context.world, move |world| {
        check_action(world, &request, &cancel)?;
        world
            .agent_finish_turn(
                request.target,
                expected.as_deref().unwrap_or(request.id.as_str()),
            )
            .map_err(|failure| error(MclErrorKind::EffectFailed, failure.to_string()))?;
        if let Some(active) = world
            .get_component_mut::<ActiveInference>(request.target)
            .filter(|active| {
                active.current.as_ref().is_some_and(|handle| {
                    completed_request.as_deref() == Some(handle.request_id.as_str())
                        && handle.turn == expected
                        && handle.is_finished()
                })
            })
        {
            active.current.take();
        }
        Ok(MclValue::Unit)
    })
    .await
}

fn explicit_turn(
    context: &CommandContext,
    parameter: Option<EffectParameter>,
) -> Result<Option<String>, MclError> {
    let Some(parameter) = parameter else {
        return Ok(None);
    };
    let EffectParameter::Manual(index) = parameter else {
        return Err(error(
            MclErrorKind::ArgumentMismatch,
            "turn must be passed explicitly with ?",
        ));
    };
    context
        .request
        .instruction
        .arguments
        .get(index)
        .and_then(Value::as_str)
        .filter(|turn| !turn.trim().is_empty())
        .map(|turn| Some(turn.to_owned()))
        .ok_or_else(|| {
            error(
                MclErrorKind::ArgumentMismatch,
                "turn must be a nonempty string",
            )
        })
}

async fn execute_inference(
    context: &CommandContext,
    messages: Vec<Value>,
    turn: Option<String>,
) -> Result<MclValue, MclError> {
    let mut typed = Vec::new();
    for message in messages {
        typed.push(inference_message(message)?);
    }
    let request = context.request.clone();
    let cancel = context.cancel.clone();
    let expected_turn = turn;
    let receiver = in_world(&context.world, move |world| {
        check_action(world, &request, &cancel)?;
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
            turn: expected_turn,
            messages: typed,
            config,
            cancel,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        Ok(receiver)
    })
    .await?;
    match receiver
        .await
        .map_err(|_| error(MclErrorKind::CleanupFailed, "inference reply was lost"))?
    {
        Ok(response) => inference_result(response, &context.request.id),
        Err(failed) => Ok(MclValue::Inference {
            message: json!({"type":"error","message":failed.to_string(),"request_id":context.request.id}),
            stop_reason: "error".into(),
        }),
    }
}

fn inference_result(response: InferenceResponse, request_id: &str) -> Result<MclValue, MclError> {
    let stop_reason = response.stop_reason.as_str().to_owned();
    let calls = response.tool_calls.into_iter().map(|call| {
        Ok(json!({"id": call.id, "tool_name": call.name, "arguments": serde_json::to_string(&call.arguments).map_err(|failed| error(MclErrorKind::EffectFailed, failed.to_string()))?}))
    }).collect::<Result<Vec<_>, MclError>>()?;
    let mut message = json!({"request_id": request_id, "type": "assistant", "reasoning": response.reasoning, "content": response.content, "tool_calls": calls, "usage": response.usage, "stop_reason": stop_reason});
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
    turn: Option<String>,
) -> Result<MclValue, MclError> {
    let calls = executable_calls(&Value::Array(messages))?;
    let request = context.request.clone();
    let cancel = context.cancel.clone();
    let expected_turn = turn;
    static NEXT_TOOL_BATCH: AtomicU64 = AtomicU64::new(1);
    let task_prefix = format!(
        "{}:{}",
        std::process::id(),
        NEXT_TOOL_BATCH.fetch_add(1, Ordering::Relaxed)
    );
    let pending = in_world(&context.world, move |world| {
        check_action(world, &request, &cancel)?;
        let mut pending = Vec::new();
        for (index, (call, failed)) in calls.into_iter().enumerate() {
            if let Some(failed) = failed {
                pending.push((call, None, Some(failed)));
                continue;
            }
            let (sender, receiver) = oneshot::channel();
            world.send_event(ToolExecuteRequest::Registered {
                id: format!("tool:{task_prefix}:{index}"),
                agent: request.target,
                turn: expected_turn.clone(),
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
    let results = futures_util::future::join_all(pending.into_iter().map(
        |(call, receiver, fallback)| async move {
            if let Some(content) = fallback {
                return tool_message(&call, content, Vec::new(), true);
            }
            match receiver.unwrap().await {
                Ok(Ok(response)) => {
                    if (!response.call_id.is_empty() && response.call_id != call.id)
                        || (!response.alias.is_empty() && response.alias != call.name)
                    {
                        tool_message(
                            &call,
                            "tool response association does not match request".into(),
                            Vec::new(),
                            true,
                        )
                    } else {
                        tool_message(&call, response.content, response.images, response.failed)
                    }
                }
                Ok(Err(failed)) => tool_message(&call, failed.to_string(), Vec::new(), true),
                Err(_) => tool_message(&call, "tool call reply was lost".into(), Vec::new(), true),
            }
        },
    ))
    .await;
    Ok(MclValue::Array(MessageArray::Message(results)))
}

fn tool_message(
    call: &InferenceToolCall,
    content: String,
    images: Vec<tool_core::ToolImage>,
    failed: bool,
) -> Value {
    json!({"type": "tool_call", "tool_call_id": call.id, "tool_name": call.name, "content": content, "data": images, "failed": failed})
}

// Validate the entire batch's identities before dispatching any side effects.
// Once a call is identifiable, malformed arguments belong to its failed tool reply.
fn executable_calls(message: &Value) -> Result<Vec<(InferenceToolCall, Option<String>)>, MclError> {
    let calls = message.as_array().ok_or_else(|| {
        error(
            MclErrorKind::InvalidMessage,
            "tool_call requires an array of calls",
        )
    })?;
    let mut unique = BTreeSet::new();
    let mut parsed = Vec::new();
    for call in calls {
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
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
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| error(MclErrorKind::InvalidMessage, "tool call name is missing"))?;
        match assistant_calls(&json!({"tool_calls": [call]})) {
            Ok(mut calls) => parsed.push((calls.remove(0), None)),
            Err(failed) => parsed.push((
                InferenceToolCall {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    arguments: Value::Null,
                },
                Some(failed.to_string()),
            )),
        }
    }
    Ok(parsed)
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
    if message.get("replay").is_some() || message.get("images").is_some() {
        return Err(error(
            MclErrorKind::InvalidMessage,
            "message contains removed replay or images fields",
        ));
    }
    let kind = message
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| error(MclErrorKind::InvalidMessage, "message type is missing"))?
        .to_owned();
    if matches!(kind.as_str(), "assistant" | "compact") {
        message["type"] = json!("assistant");
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
