use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use address::Address;
use agent_core::{Agent, AgentState};
#[cfg(test)]
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use client_core::{ClientFrame, WorldClientExt};
use core_plugin::World;
use futures_util::{FutureExt, StreamExt};
use inference_core::{
    ActiveInference, DomainRequest, InferenceError, InferenceErrorKind as Kind, InferenceHandle,
    InferenceRequest, InferenceResponse, InferenceStopReason, InferenceToolDefinition,
    InitInference, ModelRoute, ModelRoutes,
};
use lua_runtime_core::DriverLuaVm;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tool_resource_core::{read_tool, ToolReadLimits, Tools};

use crate::types::*;

fn error(kind: Kind, message: impl Into<String>) -> InferenceError {
    InferenceError::new(kind, message)
}

pub(crate) fn load_model_routes(path: &Path) -> Result<ModelRoutes, InferenceError> {
    let mut source = String::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take(MAX_CONFIG_BYTES as u64 + 1)
                .read_to_string(&mut source)
        })
        .map_err(|_| {
            error(
                Kind::InvalidModelRoute,
                "model route configuration could not be read as UTF-8",
            )
        })?;
    if source.len() > MAX_CONFIG_BYTES {
        return Err(error(
            Kind::InvalidModelRoute,
            "model route configuration exceeds budget",
        ));
    }
    let document: ModelRouteDocument = toml::from_str(&source)
        .map_err(|_| error(Kind::InvalidModelRoute, "invalid model route configuration"))?;
    if document.models.is_empty() {
        return Err(error(
            Kind::InvalidModelRoute,
            "model route configuration is empty",
        ));
    }
    let mut entries = BTreeMap::new();
    for config in document.models {
        if config.id.trim().is_empty()
            || config.id.len() > 256
            || config.id.chars().any(char::is_control)
            || config.model.trim().is_empty()
            || config.api_key.trim().is_empty()
        {
            return Err(error(
                Kind::InvalidModelRoute,
                "empty or invalid model route field",
            ));
        }
        let url = reqwest::Url::parse(&config.base_url)
            .map_err(|_| error(Kind::InvalidModelRoute, "invalid model base URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(error(
                Kind::InvalidModelRoute,
                "model base URL must be an HTTP API prefix without credentials, query or fragment",
            ));
        }
        build_adapter(
            config.provider,
            config.api_type,
            ProviderRouteInput {
                base_url: &url,
                api_key: &config.api_key,
                thinking: config.thinking.as_deref(),
                reasoning_effort: config.reasoning_effort.as_deref(),
            },
        )?;
        let context_window_tokens = parse_context_window(config.context_window.as_deref())?;
        let route = ModelRoute {
            model: config.model,
            provider: config.provider.as_str().into(),
            api_type: config.api_type.as_str().into(),
            base_url: config.base_url,
            api_key: config.api_key,
            context_window_tokens,
            thinking: config.thinking,
            reasoning_effort: config.reasoning_effort,
        };
        if entries.insert(config.id, route).is_some() {
            return Err(error(Kind::InvalidModelRoute, "duplicate model route id"));
        }
    }
    Ok(ModelRoutes { entries })
}

fn parse_context_window(value: Option<&str>) -> Result<u64, InferenceError> {
    let value = value.unwrap_or("1m").trim().to_ascii_lowercase();
    let (number, multiplier) = if let Some(value) = value.strip_suffix('k') {
        (value, 1_000u64)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 1_000_000)
    } else if let Some(value) = value.strip_suffix('b') {
        (value, 1_000_000_000)
    } else if let Some(value) = value.strip_suffix('t') {
        (value, 1_000_000_000_000)
    } else {
        return Err(error(
            Kind::InvalidModelRoute,
            "context_window requires k, m, b or t suffix",
        ));
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(multiplier))
        .filter(|&value| value > 0 && value <= i64::MAX as u64)
        .ok_or_else(|| error(Kind::InvalidModelRoute, "invalid context_window"))
}

pub(crate) fn handle_init_inference(world: &mut World, request: InitInference) {
    let Some(reply) = request
        .reply
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    else {
        return;
    };
    let result = if !world.contains_component::<Agent>(request.agent) {
        Err(error(Kind::AgentMissing, "Agent entity missing"))
    } else {
        if !world.contains_component::<ActiveInference>(request.agent) {
            world.insert_component(request.agent, ActiveInference::default());
        }
        Ok(())
    };
    let _ = reply.send(result);
}

pub(crate) fn handle_inference(world: &mut World, request: InferenceRequest) {
    let Some(reply) = request
        .reply
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    else {
        return;
    };
    let Some(agent) = world.get_component::<Agent>(request.agent) else {
        let _ = reply.send(Err(error(Kind::AgentMissing, "Agent entity missing")));
        return;
    };
    if agent.state != AgentState::Working
        || request.turn.is_empty()
        || agent.turn.as_deref() != Some(request.turn.as_str())
    {
        let _ = reply.send(Ok(InferenceResponse::cancelled()));
        return;
    }
    let prepared = (|| {
        if request.id.trim().is_empty()
            || request.id.len() > 256
            || request.id.chars().any(char::is_control)
        {
            return Err(error(Kind::InvalidRequest, "invalid inference request id"));
        }
        let active = world
            .get_component::<ActiveInference>(request.agent)
            .ok_or_else(|| error(Kind::InvalidRequest, "ActiveInference not initialized"))?;
        if active
            .current
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            return Err(error(Kind::InferenceBusy, "an inference is already active"));
        }
        let route = world
            .get_resource::<ModelRoutes>()
            .and_then(|routes| routes.get(&request.config.model))
            .ok_or_else(|| error(Kind::ModelRouteMissing, "model route not found"))?
            .clone();
        let mut seen = HashSet::new();
        let mut paths = Vec::new();
        for alias in &request.tools {
            if !seen.insert(alias) {
                return Err(error(Kind::InvalidToolDefinition, "duplicate tool alias"));
            }
            let path = world
                .get_component::<Tools>(request.agent)
                .and_then(|tools| tools.get(alias))
                .ok_or_else(|| error(Kind::ToolMissing, "tool alias not registered"))?;
            paths.push((alias.clone(), path.to_owned()));
        }
        let agent_address = world
            .get_component::<Address>(request.agent)
            .ok_or_else(|| error(Kind::InvalidRequest, "Agent address missing"))?
            .as_str()
            .to_owned();
        let vm = world
            .get_component::<DriverLuaVm>(request.agent)
            .filter(|vm| vm.instance.owner == request.agent && !vm.instance.start_id.is_empty())
            .ok_or_else(|| {
                error(
                    Kind::AgentMissing,
                    "Driver VM instance is missing or does not match Agent",
                )
            })?;
        let agent_instance = vm.instance.clone();
        let senders = world.client_webui_senders();
        let runtime = world
            .get_resource::<HttpRuntime>()
            .expect("inference HTTP runtime missing");
        Ok((
            route,
            paths,
            agent_address,
            agent_instance,
            senders,
            runtime.client.clone(),
            runtime.schedule.clone(),
        ))
    })();
    let (route, paths, agent_address, agent_instance, senders, client, schedule) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = reply.send(Err(error));
            return;
        }
    };
    let gate = Arc::new(Mutex::new(StreamGate::default()));
    let frozen = gate.clone();
    let stop_stream: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        frozen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .close()
    });
    let agent = request.agent;
    let request_id = request.id.clone();
    let turn = request.turn.clone();
    let cancel = request.cancel.clone();
    let cleanup = InferenceCleanupFinished {
        agent,
        request_id: request_id.clone(),
        instance: agent_instance.clone(),
    };
    let context = InferenceContext {
        cleanup_notification: None,
        delivery: StreamDelivery {
            id: request_id.clone(),
            agent: agent_address,
            instance: agent_instance.start_id,
            senders,
        },
        request,
        reply: Some(reply),
        route,
        tools: paths,
        client,
        gate,
        #[cfg(test)]
        cleanup_gate: None,
    };
    let handle = InferenceHandle {
        request_id,
        turn,
        cancel,
        stop_stream,
        task: Arc::new(inference_core::InferenceTaskState::default()),
    };
    world
        .get_component_mut::<ActiveInference>(agent)
        .expect("ActiveInference remains present")
        .current = Some(handle.clone());
    let launch = InferenceLaunchGuard {
        fallback: InferenceLaunchFallback {
            slot: Arc::new(Mutex::new(Some(context))),
            handle,
        },
        armed: true,
    };
    // The owned guard survives submission, dispatch, and future cancellation.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.send_async_closure(
            &schedule,
            move |runtime: async_runtime_plugin::AsyncContext| {
                // The event queue must not own a sender back into itself while queued.
                if let Some(context) = launch
                    .fallback
                    .slot
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_mut()
                {
                    context.cleanup_notification =
                        Some((InferenceCleanupSender::Async(runtime), cleanup));
                }
                launch_inference(launch)
            },
        );
    }));
}

#[derive(Clone)]
pub(crate) struct InferenceLaunchFallback {
    slot: Arc<Mutex<Option<InferenceContext>>>,
    handle: InferenceHandle,
}
impl InferenceLaunchFallback {
    pub(crate) fn fail(&self, failure: InferenceError) {
        self.handle.fail_launch(failure.clone());
        if let Some(mut context) = self
            .slot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            finish_inference(&mut context, Err(failure));
            if let Some((events, notification)) = context.cleanup_notification.take() {
                events.send_event(notification);
            }
        }
    }
}
struct InferenceLaunchGuard {
    fallback: InferenceLaunchFallback,
    armed: bool,
}
impl Drop for InferenceLaunchGuard {
    fn drop(&mut self) {
        if self.armed {
            self.fallback.fail(error(
                Kind::TaskPanicked,
                "inference launch failed before supervisor handoff",
            ));
        }
    }
}
async fn launch_inference(
    mut launch: InferenceLaunchGuard,
) -> Result<InferenceLaunchOutput, InferenceLaunchError> {
    let slot = launch.fallback.slot.clone();
    // Do not let the supervisor claim its context until the registered handle
    // owns the real task, including on a failed attach.
    let (start, started) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        if started.await.is_err() {
            return;
        }
        let context = slot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(context) = context {
            supervise_inference(context).await;
        }
    });
    launch
        .fallback
        .handle
        .attach_task(task)
        .map_err(|error| InferenceLaunchError {
            error,
            fallback: Some(launch.fallback.clone()),
        })?;
    let _ = start.send(());
    launch.armed = false;
    Ok(InferenceLaunchOutput)
}

async fn prepare_domain(context: &InferenceContext) -> Result<DomainRequest, InferenceError> {
    let paths = context.tools.clone();
    let tools = tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|(name, path)| {
                read_tool(&path, &ToolReadLimits::default())
                    .map(|input| InferenceToolDefinition {
                        name,
                        description: input.description,
                        input_schema: input.input_schema,
                    })
                    .map_err(|failure| {
                        error(
                            Kind::InvalidToolDefinition,
                            format!(
                                "tool definition read failed at {}: {:?}",
                                path.display(),
                                failure.kind
                            ),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|_| error(Kind::TaskPanicked, "tool definition reader panicked"))??;
    let mut config = context.request.config.clone();
    config.model = context.route.model.clone();
    let mut request = DomainRequest {
        messages: context.request.messages.clone(),
        tools,
        config,
    };
    normalize(&mut request)?;
    Ok(request)
}

async fn supervise_inference(mut context: InferenceContext) {
    let result = std::panic::AssertUnwindSafe(async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        if context.request.cancel.is_cancelled() || context.gate.lock().unwrap_or_else(|error| error.into_inner()).closed {
            return Ok(frozen_response(&context.gate));
        }
        let domain = prepare_domain(&context).await?;
        if context.request.cancel.is_cancelled() || context.gate.lock().unwrap_or_else(|error| error.into_inner()).closed {
            return Ok(frozen_response(&context.gate));
        }
        if context.reply.as_ref().is_some_and(|reply| reply.is_closed()) { return Err(error(Kind::ReplyClosed, "inference reply receiver closed")); }
        if tokio::time::Instant::now() >= deadline { return Err(error(Kind::Timeout, "inference preparation exhausted deadline")); }
        let names = domain.tools.iter().map(|tool| tool.name.clone()).collect::<HashSet<_>>();
        let request = build_request(domain, &context.route)?;
        let upstream = CancellationToken::new();
        let (sender, receiver) = mpsc::channel(FRAGMENT_CAPACITY);
        let provider = read_provider(context.client.clone(), request, upstream.clone(), sender);
        #[cfg(test)]
        let cleanup_gate = context.cleanup_gate.clone();
        let (completed, mut completion) = tokio::sync::oneshot::channel();
        let mut worker = tokio::spawn(async move {
            let result = provider.await;
            let _ = completed.send(result.clone());
            #[cfg(test)]
            if let Some((entered, release)) = cleanup_gate {
                entered.notify_one();
                release.notified().await;
            }
            result
        });
        let received = {
            let stream = receive_stream(context.gate.clone(), context.request.cancel.clone(), &mut context.delivery, receiver);
            tokio::pin!(stream);
            tokio::select! {
                biased;
                () = context.request.cancel.cancelled() => Err(error(Kind::Cancelled, "inference cancelled")),
                () = context.reply.as_mut().expect("inference reply remains pending").closed() => Err(error(Kind::ReplyClosed, "inference reply receiver closed")),
                () = tokio::time::sleep_until(deadline) => Err(error(Kind::Timeout, "inference execution deadline exceeded")),
                result = &mut stream => result,
            }
        };
        context.gate.lock().unwrap_or_else(|error| error.into_inner()).close();
        upstream.cancel();
        // Failure delivery must not depend on cleanup. The supervisor remains the
        // active handle until the real worker joins, preventing overlapping work.
        let known_failure = received.as_ref().err().cloned().or_else(|| {
            completion.try_recv().ok().and_then(Result::err)
        });
        if let Some(failure) = known_failure.filter(|failure| failure.kind != Kind::Cancelled) {
            finish_inference(&mut context, Err(failure));
        }
        let joined = tokio::time::timeout(Duration::from_secs(3), &mut worker).await;
        let provider = match joined {
            Ok(result) => result.map_err(|_| error(Kind::TaskPanicked, "inference HTTP worker panicked"))?,
            Err(_) => {
                let failure = error(Kind::CleanupFailed, "inference HTTP worker cleanup exceeded budget");
                if context.reply.is_some() {
                    finish_inference(&mut context, Err(failure.clone()));
                } else {
                    push_finished(&mut context.delivery, Some(&failure));
                }
                let _ = worker.await;
                return Err(failure);
            }
        };
        if received.as_ref().is_err_and(|failure| failure.kind == Kind::Cancelled) || context.request.cancel.is_cancelled() { return Ok(frozen_response(&context.gate)); }
        received?;
        provider?;
        let result = context.gate.lock().unwrap_or_else(|error| error.into_inner()).accumulator.response(&names)?;
        Ok(result)
    }).catch_unwind().await.unwrap_or_else(|_| Err(error(Kind::CleanupFailed, "inference supervisor panicked before cleanup confirmation")));
    finish_inference(&mut context, result);
    if let Some((events, notification)) = context.cleanup_notification.take() {
        events.send_event(notification);
    }
}

fn finish_inference(
    context: &mut InferenceContext,
    result: Result<InferenceResponse, InferenceError>,
) {
    context
        .gate
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .close();
    if let Some(reply) = context.reply.take() {
        push_finished(&mut context.delivery, result.as_ref().err());
        let _ = reply.send(result);
    }
}

fn frozen_response(gate: &Arc<Mutex<StreamGate>>) -> InferenceResponse {
    let mut gate = gate.lock().unwrap_or_else(|error| error.into_inner());
    gate.close();
    InferenceResponse {
        content: (!gate.accumulator.content.is_empty()).then(|| gate.accumulator.content.clone()),
        reasoning: (!gate.accumulator.reasoning.is_empty())
            .then(|| gate.accumulator.reasoning.clone()),
        tool_calls: vec![],
        replay: None,
        usage: None,
        stop_reason: InferenceStopReason::Cancelled,
    }
}

async fn receive_stream(
    gate: Arc<Mutex<StreamGate>>,
    cancel: CancellationToken,
    delivery: &mut StreamDelivery,
    mut receiver: mpsc::Receiver<ProviderFragment>,
) -> Result<(), InferenceError> {
    loop {
        let fragment = tokio::select! {
            biased;
            () = cancel.cancelled() => { gate.lock().unwrap_or_else(|error| error.into_inner()).close(); receiver.close(); return Err(error(Kind::Cancelled, "inference cancelled")); },
            fragment = receiver.recv() => fragment,
        };
        let Some(fragment) = fragment else {
            return Ok(());
        };
        let delta = {
            let mut gate = gate.lock().unwrap_or_else(|error| error.into_inner());
            if gate.closed || cancel.is_cancelled() {
                gate.close();
                receiver.close();
                return Err(error(Kind::Cancelled, "inference stream frozen"));
            }
            let delta = gate.accumulator.accept(fragment)?;
            delta.map(|(reasoning, content)| {
                gate.sequence += 1;
                (gate.sequence, reasoning, content)
            })
        };
        if let Some((sequence, reasoning, content)) = delta {
            push_stream(delivery, sequence, reasoning, content);
        }
    }
}

async fn read_provider(
    client: reqwest::Client,
    request: ProviderHttpRequest,
    cancel: CancellationToken,
    sender: mpsc::Sender<ProviderFragment>,
) -> Result<(), InferenceError> {
    read_provider_with_timeouts(
        client,
        request,
        cancel,
        sender,
        (Duration::from_secs(180), Duration::from_secs(60)),
    )
    .await
}

async fn read_provider_with_timeouts(
    client: reqwest::Client,
    request: ProviderHttpRequest,
    cancel: CancellationToken,
    sender: mpsc::Sender<ProviderFragment>,
    deadlines: (Duration, Duration),
) -> Result<(), InferenceError> {
    let secrets = provider_error_secrets(&request.headers);
    let mut outbound = reqwest::Request::new(request.method, request.url);
    *outbound.headers_mut() = request.headers;
    *outbound.body_mut() = Some(reqwest::Body::from(request.body));
    let response = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(error(Kind::Cancelled, "upstream cancelled")),
        result = tokio::time::timeout(deadlines.0, client.execute(outbound)) =>
            result.map_err(|_| error(Kind::Timeout, "provider response headers timed out"))?
                .map_err(http_request_error)?,
    };
    if !response.status().is_success() {
        return Err(read_provider_status_error(response, &cancel, deadlines.1, &secrets).await);
    }
    let mut decoder = request.decoder;
    let mut body = response.bytes_stream();
    loop {
        let chunk = tokio::select! { biased; () = cancel.cancelled() => return Err(error(Kind::Cancelled, "upstream cancelled")), chunk = tokio::time::timeout(deadlines.1, body.next()) => chunk.map_err(|_| error(Kind::Timeout, "provider stream idle timeout"))? };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|failure| {
            if failure.is_timeout() {
                error(Kind::Timeout, "provider stream idle timeout")
            } else {
                error(Kind::ResponseIncomplete, "provider stream disconnected")
            }
        })?;
        for fragment in decoder.push(&chunk)? {
            tokio::select! { biased; () = cancel.cancelled() => return Err(error(Kind::Cancelled, "upstream cancelled")), result = sender.send(fragment) => if result.is_err() { return Err(error(Kind::Cancelled, "fragment receiver closed")); } }
        }
    }
    for fragment in decoder.finish()? {
        tokio::select! { biased; () = cancel.cancelled() => return Err(error(Kind::Cancelled, "upstream cancelled")), result = sender.send(fragment) => if result.is_err() { return Err(error(Kind::Cancelled, "fragment receiver closed")); } }
    }
    Ok(())
}

const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_STATUS_ERROR_BYTES: usize = 4096;

fn provider_error_secrets(headers: &reqwest::header::HeaderMap) -> Vec<String> {
    let mut secrets = Vec::new();
    for name in ["authorization", "x-api-key"] {
        for value in headers.get_all(name) {
            if let Ok(value) = value.to_str() {
                if !value.is_empty() {
                    secrets.push(value.to_owned());
                    if name == "authorization" {
                        if let Some((_, credential)) = value.split_once(char::is_whitespace) {
                            if !credential.trim().is_empty() {
                                secrets.push(credential.trim().to_owned());
                            }
                        }
                    }
                }
            }
        }
    }
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    secrets
}

fn safe_error_field(value: &str, secrets: &[String], limit: usize) -> String {
    // Redact before truncation so a credential crossing the budget is never exposed.
    let mut value = value.to_owned();
    for secret in secrets {
        value = value.replace(secret, "[REDACTED]");
    }
    let mut safe = String::new();
    for word in value.split_whitespace() {
        if !safe.is_empty() {
            safe.push(' ');
        }
        let lower = word.to_ascii_lowercase();
        if lower.contains("http://") || lower.contains("https://") {
            safe.push_str("[URL REDACTED]");
        } else {
            safe.extend(word.chars().filter(|ch| !ch.is_control()));
        }
    }
    if safe.chars().count() > limit {
        safe = safe.chars().take(limit.saturating_sub(1)).collect();
        safe.push('…');
    }
    safe
}

async fn read_provider_status_error(
    response: reqwest::Response,
    cancel: &CancellationToken,
    read_deadline: Duration,
    secrets: &[String],
) -> InferenceError {
    let status = response.status();
    let headers = response.headers().clone();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    let read = async {
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(_) => return Some("error body disconnected"),
            };
            let remaining = MAX_ERROR_BODY_BYTES - body.len();
            body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            if body.len() == MAX_ERROR_BODY_BYTES {
                return Some("error body truncated");
            }
        }
        None
    };
    // Use the existing read budget for the entire error read, including trickles.
    let diagnostic = tokio::select! {
        biased;
        () = cancel.cancelled() => return error(Kind::Cancelled, "upstream cancelled"),
        result = tokio::time::timeout(read_deadline, read) => match result {
            Ok(diagnostic) => diagnostic,
            Err(_) => Some("error body read timed out"),
        },
    };
    format_provider_status_error(status, &headers, &body, diagnostic, secrets)
}

fn format_provider_status_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
    diagnostic: Option<&str>,
    secrets: &[String],
) -> InferenceError {
    let mut output = format!(
        "HTTP {} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Unknown Status")
    );
    let parsed = serde_json::from_slice::<serde_json::Value>(body).ok();
    let upstream = parsed.as_ref().and_then(|value| value.get("error"));
    let field = |name: &str| {
        upstream
            .and_then(|value| value.get(name))
            .and_then(|value| match value {
                serde_json::Value::String(value) => Some(value.clone()),
                serde_json::Value::Number(value) => Some(value.to_string()),
                _ => None,
            })
    };
    let request_id = field("request_id")
        .or_else(|| {
            parsed
                .as_ref()?
                .get("request_id")?
                .as_str()
                .map(str::to_owned)
        })
        .or_else(|| {
            ["x-request-id", "request-id"].into_iter().find_map(|name| {
                headers
                    .get(name)?
                    .to_str()
                    .ok()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
        });
    let message = field("message").or_else(|| upstream?.as_str().map(str::to_owned));
    let request_id = request_id.or_else(|| {
        let message = message.as_deref()?;
        let (_, tail) = message.split_once("(request_id:")?;
        let (id, _) = tail.split_once(')')?;
        (!id.trim().is_empty()).then(|| id.trim().to_owned())
    });
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let text = String::from_utf8_lossy(body);
    let looks_structured = matches!(text.trim_start().chars().next(), Some('{' | '[' | '<'));
    let message = message.or_else(|| {
        (parsed.is_none()
            && !body.is_empty()
            && !looks_structured
            && (content_type.is_empty() || content_type.starts_with("text/plain")))
        .then(|| safe_error_field(&text, secrets, 512))
    });
    // Reserve most of the budget for request correlation and message detail.
    for (name, value, limit) in [
        ("type", field("type"), 48),
        ("code", field("code"), 48),
        ("request_id", request_id, 96),
    ] {
        if let Some(value) = value {
            let value = safe_error_field(&value, secrets, limit);
            if !value.is_empty() {
                output.push_str(&format!("; {name}={value}"));
            }
        }
    }
    let detail = message.unwrap_or_else(|| {
        if body.is_empty() {
            "empty error response".to_owned()
        } else {
            "non-JSON error response".to_owned()
        }
    });
    let suffix = diagnostic
        .map(|value| format!("; {value}"))
        .unwrap_or_default();
    let media_type = content_type.split(';').next().unwrap_or("");
    let suffix = if parsed.is_none() && looks_structured && !media_type.is_empty() {
        format!(
            "; content_type={}{}",
            safe_error_field(media_type, secrets, 48),
            suffix
        )
    } else {
        suffix
    };
    let budget = MAX_STATUS_ERROR_BYTES.saturating_sub(output.len() + 10 + suffix.len());
    let mut detail = safe_error_field(&detail, secrets, MAX_STATUS_ERROR_BYTES);
    if detail.len() > budget {
        let mut end = budget.saturating_sub('…'.len_utf8());
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        detail.truncate(end);
        detail.push('…');
    }
    output.push_str("; message=");
    output.push_str(&detail);
    output.push_str(&suffix);
    error(Kind::ResponseStatus, output)
}

fn push_stream(delivery: &mut StreamDelivery, sequence: u64, reasoning: bool, content: String) {
    let frame = json!({"type":if reasoning { "agent.message.reasoning_delta" } else { "agent.message.delta" },"id":delivery.id,"agent":delivery.agent,"agent_instance":delivery.instance,"sequence":sequence,"content":content});
    if let Ok(encoded) = serde_json::to_string(&frame) {
        delivery.senders.retain(|sender| {
            !sender.is_closed() && sender.try_send(ClientFrame::Text(encoded.clone())).is_ok()
        });
    }
}

fn push_finished(delivery: &mut StreamDelivery, failure: Option<&InferenceError>) {
    let frame = json!({"type":"agent.message.finished","id":delivery.id,"agent":delivery.agent,"agent_instance":delivery.instance,"error":failure.map(|failure| json!({"kind":format!("{:?}",failure.kind),"message":failure.message}))});
    if let Ok(encoded) = serde_json::to_string(&frame) {
        delivery.senders.retain(|sender| {
            !sender.is_closed() && sender.try_send(ClientFrame::Text(encoded.clone())).is_ok()
        });
    }
}

#[cfg(test)]
#[path = "../tests/internal/stream.rs"]
mod tests;

fn http_request_error(failure: reqwest::Error) -> InferenceError {
    if failure.is_timeout() {
        return error(
            Kind::Timeout,
            "provider did not respond before the HTTP deadline",
        );
    }
    let mut source = std::error::Error::source(&failure);
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            let description = match io.kind() {
                std::io::ErrorKind::ConnectionRefused => "provider connection refused",
                std::io::ErrorKind::ConnectionReset => "provider connection reset",
                std::io::ErrorKind::ConnectionAborted => "provider connection aborted",
                std::io::ErrorKind::TimedOut => "provider connection timed out",
                _ => "provider connection failed",
            };
            return error(Kind::RequestFailed, description);
        }
        source = cause.source();
    }
    error(Kind::RequestFailed, "inference HTTP request failed")
}
