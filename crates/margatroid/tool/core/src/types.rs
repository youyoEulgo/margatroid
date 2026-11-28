use crate::{ToolError, ToolErrorKind, encoded_len};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskListEntry {
    pub task_id: String,
    pub tool_name: String,
    pub has_unread_output: bool,
    pub error: Option<ToolError>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskReadResult {
    pub replaced: bool,
    pub output: Vec<Value>,
    pub error: Option<ToolError>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolTaskSnapshot {
    pub task_id: String,
    pub call_id: String,
    pub alias: String,
    pub returned: bool,
    pub output: Vec<Value>,
    pub error: Option<ToolError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolExecutionLimits {
    pub lua_vm_memory_bytes: usize,
    pub cache_bytes: usize,
}

impl Default for ToolExecutionLimits {
    fn default() -> Self {
        Self {
            lua_vm_memory_bytes: 2 * 1024 * 1024 * 1024,
            cache_bytes: 128 * 1024 * 1024,
        }
    }
}

impl ToolExecutionLimits {
    pub fn validate(&self) -> Result<(), ToolError> {
        if self.lua_vm_memory_bytes == 0 || self.cache_bytes == 0 {
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

pub type TaskChildCloseFuture = Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send>>;
pub type TaskChildClose = Arc<dyn Fn() -> TaskChildCloseFuture + Send + Sync>;
pub type TaskFinish = Arc<dyn Fn(ToolTaskHandle) -> TaskChildCloseFuture + Send + Sync>;
#[derive(Default)]
pub(crate) struct TaskChildren {
    accepting: bool,
    completed: bool,
    children: BTreeMap<core_plugin::Entity, TaskChildClose>,
    retries: BTreeMap<u64, TaskChildClose>,
    next_retry: u64,
}
impl std::fmt::Debug for TaskChildren {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskChildren")
            .field("children", &self.children.len())
            .field("retries", &self.retries.len())
            .finish()
    }
}

struct ToolTaskShared {
    execution_id: String,
    turn: Option<String>,
    registrations: crate::TaskScope,
    children: Mutex<TaskChildren>,
    child_cleanup: tokio::sync::Mutex<()>,
    retry_cleanup: tokio::sync::Mutex<()>,
    finish: TaskFinish,
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    task_attached: AtomicBool,
    supervisor_ended: AtomicBool,
    shared: Mutex<ToolTaskData>,
    input: mpsc::UnboundedSender<Value>,
    changed: Notify,
    cache_bytes: usize,
}

impl std::fmt::Debug for ToolTaskShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolTaskShared")
            .field("execution_id", &self.execution_id)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug)]
pub struct ToolTaskHandle {
    inner: Arc<ToolTaskShared>,
    updates: crate::TaskUpdateContext,
}

impl ToolTaskHandle {
    pub fn turn(&self) -> Option<&str> {
        self.inner.turn.as_deref()
    }
    pub(crate) fn update_context(&self) -> &crate::TaskUpdateContext {
        &self.updates
    }
    pub fn new(
        limits: &ToolExecutionLimits,
        call_id: String,
        alias: String,
        reply: oneshot::Sender<Result<ToolExecuteResponse, ToolError>>,
        updates: crate::TaskUpdateContext,
        turn: Option<String>,
        finish: TaskFinish,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Value>), ToolError> {
        if let Err(error) = limits.validate() {
            let _ = reply.send(Err(error.clone()));
            return Err(error);
        }
        static NEXT_EXECUTION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let execution_id = format!(
            "task-instance:{}:{}",
            std::process::id(),
            NEXT_EXECUTION.fetch_add(1, Ordering::Relaxed)
        );
        let (input, receiver) = mpsc::unbounded_channel();
        Ok((
            Self {
                updates,
                inner: Arc::new(ToolTaskShared {
                    execution_id,
                    turn,
                    registrations: crate::TaskScope::default(),
                    children: Mutex::new(TaskChildren {
                        accepting: true,
                        ..Default::default()
                    }),
                    child_cleanup: tokio::sync::Mutex::new(()),
                    retry_cleanup: tokio::sync::Mutex::new(()),
                    finish,
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

    pub fn execution_id(&self) -> &str {
        &self.inner.execution_id
    }
    pub fn registration_scope(&self) -> crate::TaskScope {
        self.inner.registrations.clone()
    }
    pub fn close_registrations(&self) {
        self.inner.registrations.close();
    }
    pub fn cancel(&self) {
        lock(&self.inner.children).accepting = false;
        self.close_registrations();
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
        crate::sync::update_task(&self.updates, || {
            let mut state = lock(&self.inner.shared);
            ensure_open(&state)?;
            if state.returned {
                return Err(ToolError::new(
                    ToolErrorKind::AlreadyReturned,
                    "tool call already returned",
                ));
            }
            deliver_reply(&mut state, result);
            Ok(((), true))
        })
    }

    pub(crate) fn abort_unreturned(&self) -> bool {
        crate::sync::update_task(&self.updates, || {
            let mut state = lock(&self.inner.shared);
            if state.cleanup_complete || state.returned {
                return Ok((false, false));
            }
            let response = ToolExecuteResponse::aborted(state.call_id.clone(), state.alias.clone());
            deliver_reply(&mut state, Ok(response));
            self.cancel();
            Ok((true, true))
        })
        .unwrap_or(false)
    }

    pub fn has_returned(&self) -> bool {
        lock(&self.inner.shared).returned
    }

    pub fn same_task(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn write(&self, value: Value) -> Result<(), ToolError> {
        crate::sync::update_task(&self.updates, || {
            let mut state = lock(&self.inner.shared);
            ensure_open(&state)?;
            append_cache(&mut state, value, self.inner.cache_bytes);
            Ok(((), true))
        })
    }

    pub fn replace(&self, value: Value) -> Result<(), ToolError> {
        crate::sync::update_task(&self.updates, || {
            let mut state = lock(&self.inner.shared);
            ensure_open(&state)?;
            state.output.clear();
            state.bytes = 0;
            state.cursor = 0;
            state.replaced = true;
            append_cache(&mut state, value, self.inner.cache_bytes);
            Ok(((), true))
        })
    }

    /// Metadata-only listing does not copy or consume the output cache.
    pub fn list_entry(&self, task_id: &str) -> Option<TaskListEntry> {
        let state = lock(&self.inner.shared);
        if !state.returned || state.cleanup_complete {
            return None;
        }
        Some(TaskListEntry {
            task_id: task_id.to_owned(),
            tool_name: state.alias.clone(),
            has_unread_output: state.replaced || state.cursor < state.output.len(),
            error: state.error.clone(),
        })
    }

    pub fn snapshot(&self, task_id: &str) -> Option<ToolTaskSnapshot> {
        let state = lock(&self.inner.shared);
        if state.cleanup_complete {
            return None;
        }
        Some(ToolTaskSnapshot {
            task_id: task_id.to_owned(),
            call_id: state.call_id.clone(),
            alias: state.alias.clone(),
            returned: state.returned,
            output: state.output.clone(),
            error: state.error.clone(),
        })
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
        let _ = crate::sync::update_task(&self.updates, || {
            let mut state = lock(&self.inner.shared);
            let changed = state.error.as_ref() != Some(&error);
            state.error = Some(error);
            Ok(((), changed))
        });
        self.inner.changed.notify_waiters();
    }

    pub(crate) fn complete_cleanup_raw(&self, error: Option<ToolError>) {
        self.close_registrations();
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

    pub fn finish(&self) -> TaskChildCloseFuture {
        self.cancel();
        if self.is_cleanup_complete() {
            return Box::pin(async { Ok(()) });
        }
        (self.inner.finish)(self.clone())
    }
    pub fn bind_child(
        &self,
        entity: core_plugin::Entity,
        close: TaskChildClose,
    ) -> Result<(), ToolError> {
        let mut state = lock(&self.inner.children);
        if state.completed || !state.accepting || self.inner.cancel.is_cancelled() {
            return Err(ToolError::new(
                ToolErrorKind::Closed,
                "task no longer accepts children",
            ));
        }
        state.children.entry(entity).or_insert(close);
        Ok(())
    }
    /// Clone an already registered child's opaque close capability for failed
    /// creation handoff compensation. This does not reopen child admission.
    pub fn child_close(&self, entity: core_plugin::Entity) -> Option<TaskChildClose> {
        lock(&self.inner.children).children.get(&entity).cloned()
    }

    pub fn retain_child_for_cleanup(
        &self,
        entity: core_plugin::Entity,
        close: TaskChildClose,
    ) -> Result<(), ToolError> {
        let mut state = lock(&self.inner.children);
        if state.completed {
            return Err(ToolError::new(
                ToolErrorKind::Closed,
                "task already cleaned up",
            ));
        }
        state.children.entry(entity).or_insert(close);
        Ok(())
    }
    pub async fn finish_children(&self) -> Result<(), ToolError> {
        let _run = self.inner.child_cleanup.lock().await;
        let children = {
            let mut state = lock(&self.inner.children);
            state.accepting = false;
            state
                .children
                .iter()
                .map(|(entity, close)| (*entity, close.clone()))
                .collect::<Vec<_>>()
        };
        let mut failure = None;
        for (entity, close) in children {
            match close().await {
                Ok(()) => {
                    lock(&self.inner.children).children.remove(&entity);
                }
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
    pub fn retain_cleanup(&self, close: TaskChildClose) -> Result<(), ToolError> {
        let mut state = lock(&self.inner.children);
        if state.completed {
            return Err(ToolError::new(
                ToolErrorKind::Closed,
                "task already cleaned up",
            ));
        }
        let id = state.next_retry;
        state.next_retry = id
            .checked_add(1)
            .ok_or_else(|| ToolError::new(ToolErrorKind::CleanupFailed, "cleanup id overflow"))?;
        state.retries.insert(id, close);
        Ok(())
    }
    pub async fn retry_cleanups(&self) -> Result<(), ToolError> {
        let _run = self.inner.retry_cleanup.lock().await;
        let retries = lock(&self.inner.children)
            .retries
            .iter()
            .map(|(id, close)| (*id, close.clone()))
            .collect::<Vec<_>>();
        let mut failure = None;
        for (id, close) in retries {
            match close().await {
                Ok(()) => {
                    lock(&self.inner.children).retries.remove(&id);
                }
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
    pub(crate) fn begin_cleanup_completion(&self) -> bool {
        let mut state = lock(&self.inner.children);
        if !state.children.is_empty() || !state.retries.is_empty() {
            return false;
        }
        state.completed = true;
        state.accepting = false;
        true
    }
    /// Wait only for the execution supervisor to exit; child and retry cleanup
    /// remains the responsibility of the independently supervised TaskFinish.
    pub async fn wait_execution_ended(&self) -> Result<(), ToolError> {
        if let Some(current) = tokio::task::try_id()
            && lock(&self.inner.task)
                .as_ref()
                .is_some_and(|task| task.id() == current)
        {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "supervisor cannot join itself",
            ));
        }
        loop {
            // Register before observing the attachment slot: attach_task may run
            // after this waiter begins, and notify_waiters does not retain permits.
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.inner.supervisor_ended.load(Ordering::Acquire) || self.is_cleanup_complete() {
                return Ok(());
            }
            let joined = poll_fn(|cx| {
                let mut slot = lock(&self.inner.task);
                let Some(task) = slot.as_mut() else {
                    return Poll::Pending;
                };
                match Pin::new(task).poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(result) => {
                        *slot = None;
                        self.inner.supervisor_ended.store(true, Ordering::Release);
                        self.inner.changed.notify_waiters();
                        Poll::Ready(result.map_err(|failure| {
                            ToolError::new(ToolErrorKind::TaskPanicked, failure.to_string())
                        }))
                    }
                }
            });
            tokio::select! {
                result = joined => return result,
                () = &mut changed => {},
            }
        }
    }

    /// Observe the execution supervisor without invoking the finish callback again.
    pub async fn wait_finished(&self) -> Result<(), ToolError> {
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
