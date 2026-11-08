use crate::{ToolError, ToolErrorKind, encoded_len};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolImage {
    pub media_type: String,
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecuteResponse {
    pub call_id: String,
    pub alias: String,
    pub content: String,
    pub images: Vec<ToolImage>,
    pub failed: bool,
}

impl ToolExecuteResponse {
    pub fn aborted(call_id: String, alias: String) -> Self {
        Self {
            call_id,
            alias,
            content: "Tool call aborted.".into(),
            images: Vec::new(),
            failed: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskReadResult {
    pub replaced: bool,
    pub output: Vec<Value>,
    pub error: Option<ToolError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolExecutionLimits {
    pub total_memory_bytes: usize,
    pub memory_bytes: usize,
    pub cache_bytes: usize,
}

impl Default for ToolExecutionLimits {
    fn default() -> Self {
        Self {
            total_memory_bytes: 8 * 1024 * 1024 * 1024,
            memory_bytes: 2 * 1024 * 1024 * 1024,
            cache_bytes: 128 * 1024 * 1024,
        }
    }
}

impl ToolExecutionLimits {
    pub fn validate(&self) -> Result<(), ToolError> {
        if self.total_memory_bytes == 0 || self.memory_bytes == 0 || self.cache_bytes == 0 {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "invalid tool execution limits",
            ));
        }
        Ok(())
    }
}

/// Per-invocation lifetime returned by Lua configure; zero means unlimited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolConfiguration {
    pub timeout_ms: u64,
}

impl ToolConfiguration {
    pub fn validate(&self) -> Result<(), ToolError> {
        if self.timeout_ms != 0
            && std::time::Instant::now()
                .checked_add(Duration::from_millis(self.timeout_ms))
                .is_none()
        {
            return Err(ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "tool timeout_ms must be representable integer milliseconds",
            ));
        }
        Ok(())
    }

    pub fn timeout(&self) -> Option<Duration> {
        (self.timeout_ms != 0).then(|| Duration::from_millis(self.timeout_ms))
    }
}

#[derive(Debug)]
struct ToolTaskData {
    lifetime: Option<Option<tokio::time::Instant>>,
    output: Vec<Value>,
    bytes: usize,
    cursor: usize,
    replaced: bool,
    cleanup_complete: bool,
    returned: bool,
    call_id: String,
    alias: String,
    reply: Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>,
    error: Option<ToolError>,
}

#[derive(Debug)]
struct ToolTaskShared {
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    task_attached: AtomicBool,
    supervisor_ended: AtomicBool,
    shared: Mutex<ToolTaskData>,
    input: mpsc::UnboundedSender<Value>,
    changed: Notify,
    cache_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ToolTaskHandle {
    inner: Arc<ToolTaskShared>,
}

impl ToolTaskHandle {
    pub fn new(
        limits: &ToolExecutionLimits,
        call_id: String,
        alias: String,
        reply: oneshot::Sender<Result<ToolExecuteResponse, ToolError>>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Value>), ToolError> {
        if let Err(error) = limits.validate() {
            let _ = reply.send(Err(error.clone()));
            return Err(error);
        }
        let (input, receiver) = mpsc::unbounded_channel();
        Ok((
            Self {
                inner: Arc::new(ToolTaskShared {
                    cancel: CancellationToken::new(),
                    task: Mutex::new(None),
                    task_attached: AtomicBool::new(false),
                    supervisor_ended: AtomicBool::new(false),
                    shared: Mutex::new(ToolTaskData {
                        lifetime: None,
                        output: Vec::new(),
                        bytes: 0,
                        cursor: 0,
                        replaced: false,
                        cleanup_complete: false,
                        returned: false,
                        call_id,
                        alias,
                        reply: Some(reply),
                        error: None,
                    }),
                    input,
                    changed: Notify::new(),
                    cache_bytes: limits.cache_bytes,
                }),
            },
            receiver,
        ))
    }

    /// Confirm the lifetime once, beginning at configuration acceptance.
    pub fn configure_lifetime(&self, timeout_ms: u64) -> Result<(), ToolError> {
        let configuration = ToolConfiguration { timeout_ms };
        configuration.validate()?;
        let mut state = lock(&self.inner.shared);
        ensure_open(&state)?;
        if state.lifetime.is_some() {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "tool task lifetime already configured",
            ));
        }
        let deadline = configuration
            .timeout()
            .map(|duration| {
                tokio::time::Instant::now()
                    .checked_add(duration)
                    .ok_or_else(|| {
                        ToolError::new(
                            ToolErrorKind::InvalidDefinition,
                            "tool task lifetime is unrepresentable",
                        )
                    })
            })
            .transpose()?;
        state.lifetime = Some(deadline);
        drop(state);
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Wait for the configured deadline; configuration and unlimited tasks have no default clock.
    pub async fn lifetime_expired(&self) {
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let deadline = lock(&self.inner.shared).lifetime.flatten();
            tokio::select! {
                () = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => return,
                () = &mut changed => {},
            }
        }
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.inner.cancel.clone()
    }

    pub fn cancel(&self) {
        self.inner.cancel.cancel();
    }

    pub fn attach_task(&self, task: JoinHandle<()>) -> Result<(), ToolError> {
        let mut slot = lock(&self.inner.task);
        if self.inner.task_attached.swap(true, Ordering::AcqRel) {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "supervisor already attached",
            ));
        }
        *slot = Some(task);
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Atomically claim and deliver the invocation's sole reply.
    pub fn return_result(
        &self,
        result: Result<ToolExecuteResponse, ToolError>,
    ) -> Result<(), ToolError> {
        let mut state = lock(&self.inner.shared);
        ensure_open(&state)?;
        if state.returned {
            return Err(ToolError::new(
                ToolErrorKind::AlreadyReturned,
                "tool call already returned",
            ));
        }
        deliver_reply(&mut state, result);
        Ok(())
    }

    pub(crate) fn abort_unreturned(&self) -> bool {
        let mut state = lock(&self.inner.shared);
        if state.cleanup_complete || state.returned {
            return false;
        }
        let response = ToolExecuteResponse::aborted(state.call_id.clone(), state.alias.clone());
        deliver_reply(&mut state, Ok(response));
        self.inner.cancel.cancel();
        true
    }

    pub fn has_returned(&self) -> bool {
        lock(&self.inner.shared).returned
    }

    pub(crate) fn same_task(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn write(&self, value: Value) -> Result<(), ToolError> {
        let mut state = lock(&self.inner.shared);
        ensure_open(&state)?;
        append_cache(&mut state, value, self.inner.cache_bytes);
        Ok(())
    }

    pub fn replace(&self, value: Value) -> Result<(), ToolError> {
        let mut state = lock(&self.inner.shared);
        ensure_open(&state)?;
        state.output.clear();
        state.bytes = 0;
        state.cursor = 0;
        state.replaced = true;
        append_cache(&mut state, value, self.inner.cache_bytes);
        Ok(())
    }

    pub fn read(&self) -> Result<TaskReadResult, ToolError> {
        let mut state = lock(&self.inner.shared);
        ensure_open(&state)?;
        let mut output = Vec::new();
        for value in &state.output[state.cursor..] {
            match (output.last_mut(), value) {
                (Some(Value::String(text)), Value::String(fragment)) => text.push_str(fragment),
                _ => output.push(value.clone()),
            }
        }
        let result = TaskReadResult {
            replaced: state.replaced,
            output,
            error: state.error.clone(),
        };
        state.cursor = state.output.len();
        state.replaced = false;
        Ok(result)
    }

    pub fn send(&self, value: Value) -> Result<(), ToolError> {
        let state = lock(&self.inner.shared);
        ensure_open(&state)?;
        self.inner
            .input
            .send(value)
            .map_err(|_| ToolError::new(ToolErrorKind::InputClosed, "tool input is closed"))
    }

    pub fn set_error(&self, error: ToolError) {
        lock(&self.inner.shared).error = Some(error);
        self.inner.changed.notify_waiters();
    }

    pub(crate) fn complete_cleanup(&self, error: Option<ToolError>) {
        let mut state = lock(&self.inner.shared);
        if state.cleanup_complete {
            return;
        }
        if let Some(error) = error {
            state.error = Some(error);
        }
        state.output.clear();
        state.output.shrink_to_fit();
        state.bytes = 0;
        state.cursor = 0;
        state.replaced = false;
        state.cleanup_complete = true;
        drop(state);
        self.inner.changed.notify_waiters();
    }

    pub fn is_cleanup_complete(&self) -> bool {
        lock(&self.inner.shared).cleanup_complete
    }

    pub async fn finish(&self) -> Result<(), ToolError> {
        // A supervisor must exit through its cleanup path, never join itself.
        if let Some(current) = tokio::task::try_id()
            && lock(&self.inner.task)
                .as_ref()
                .is_some_and(|task| task.id() == current)
        {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "supervisor cannot finish itself",
            ));
        }
        self.cancel();
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_cleanup_complete() {
                return Ok(());
            }
            if let Some(error) = lock(&self.inner.shared)
                .error
                .as_ref()
                .filter(|error| error.kind == ToolErrorKind::CleanupFailed)
                .cloned()
            {
                return Err(error);
            }
            if self.inner.supervisor_ended.load(Ordering::Acquire) {
                return Err(self.finish_error());
            }
            let joined = poll_fn(|context| {
                let mut slot = lock(&self.inner.task);
                if let Some(task) = slot.as_mut() {
                    match Pin::new(task).poll(context) {
                        Poll::Ready(result) => {
                            *slot = None;
                            if let Err(error) = result {
                                self.set_error(ToolError::new(
                                    ToolErrorKind::TaskPanicked,
                                    format!("tool supervisor failed: {error}"),
                                ));
                            }
                            self.inner.supervisor_ended.store(true, Ordering::Release);
                            self.inner.changed.notify_waiters();
                            Poll::Ready(())
                        }
                        Poll::Pending => Poll::Pending,
                    }
                } else if self.inner.supervisor_ended.load(Ordering::Acquire) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            });
            tokio::select! {
                () = joined => {},
                () = &mut changed => {},
            }
        }
    }

    fn finish_error(&self) -> ToolError {
        lock(&self.inner.shared).error.clone().unwrap_or_else(|| {
            ToolError::new(
                ToolErrorKind::CleanupFailed,
                "supervisor ended without confirming tool cleanup",
            )
        })
    }
}

/// Keep a serialized byte window without changing explicit replacement semantics.
fn append_cache(state: &mut ToolTaskData, mut value: Value, limit: usize) {
    let bytes = match encoded_len(&value, limit) {
        Ok(bytes) => bytes,
        Err(_) => {
            let Value::String(text) = &value else {
                return;
            };
            if limit < 2 {
                return;
            }
            // JSON strings include two quotes and escape control characters.
            let mut start = text.len();
            let mut used = 2;
            for (index, character) in text.char_indices().rev() {
                let cost = match character {
                    '"' | '\\' | '\x08' | '\t' | '\n' | '\x0c' | '\r' => 2,
                    '\x00'..='\x1f' => 6,
                    character => character.len_utf8(),
                };
                if cost > limit - used {
                    break;
                }
                used += cost;
                start = index;
            }
            value = Value::String(text[start..].to_owned());
            encoded_len(&value, limit).expect("fitting cache suffix")
        }
    };
    let mut removed = 0;
    while state.bytes > limit - bytes {
        state.bytes -=
            encoded_len(&state.output[removed], limit).expect("cached value fits window");
        removed += 1;
    }
    if removed != 0 {
        state.output.drain(..removed);
        state.cursor = state.cursor.saturating_sub(removed);
    }
    state.bytes += bytes;
    state.output.push(value);
}

/// All replies for a registered task pass through this shared-lock operation.
fn deliver_reply(state: &mut ToolTaskData, result: Result<ToolExecuteResponse, ToolError>) {
    let result = result.map(|mut response| {
        response.call_id = state.call_id.clone();
        response.alias = state.alias.clone();
        response
    });
    state.returned = true;
    if let Some(reply) = state.reply.take() {
        let _ = reply.send(result);
    }
}

fn ensure_open(state: &ToolTaskData) -> Result<(), ToolError> {
    if state.cleanup_complete {
        Err(ToolError::new(
            ToolErrorKind::TaskMissing,
            "tool task has been cleaned up",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
