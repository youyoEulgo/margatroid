use crate::{ToolError, ToolErrorKind, encoded_len};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskReadResult {
    pub replaced: bool,
    pub output: Vec<Value>,
    pub completed: bool,
    pub error: Option<ToolError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolExecutionLimits {
    pub script_bytes: usize,
    pub argument_bytes: usize,
    pub output_bytes: usize,
    pub result_bytes: usize,
    pub diagnostic_bytes: usize,
    pub memory_bytes: usize,
    pub instruction_budget: u64,
    pub execution_timeout: Option<Duration>,
    pub host_call_timeout: Duration,
    pub cleanup_timeout: Duration,
    pub cache_bytes: usize,
    pub input_capacity: usize,
}

impl Default for ToolExecutionLimits {
    fn default() -> Self {
        Self {
            script_bytes: 1024 * 1024,
            argument_bytes: 256 * 1024,
            output_bytes: 1024 * 1024,
            result_bytes: 32 * 1024 * 1024,
            diagnostic_bytes: 256 * 1024,
            memory_bytes: 64 * 1024 * 1024,
            instruction_budget: 10_000_000,
            execution_timeout: None,
            host_call_timeout: Duration::from_secs(10),
            cleanup_timeout: Duration::from_secs(3),
            cache_bytes: 4 * 1024 * 1024,
            input_capacity: 32,
        }
    }
}

impl ToolExecutionLimits {
    pub fn validate(&self) -> Result<(), ToolError> {
        if [
            self.script_bytes,
            self.argument_bytes,
            self.output_bytes,
            self.result_bytes,
            self.diagnostic_bytes,
            self.memory_bytes,
            self.cache_bytes,
            self.input_capacity,
        ]
        .contains(&0)
            || self.input_capacity > tokio::sync::Semaphore::MAX_PERMITS
            || self.instruction_budget == 0
            || self.execution_timeout.is_some_and(|timeout| {
                timeout.is_zero() || std::time::Instant::now().checked_add(timeout).is_none()
            })
            || self.host_call_timeout.is_zero()
            || self.cleanup_timeout.is_zero()
            || self
                .execution_timeout
                .is_some_and(|timeout| self.host_call_timeout > timeout)
        {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "invalid tool execution limits",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ToolTaskData {
    output: Vec<Value>,
    bytes: usize,
    cursor: usize,
    replaced: bool,
    completed: bool,
    error: Option<ToolError>,
}

#[derive(Debug)]
struct ToolTaskShared {
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    task_attached: AtomicBool,
    supervisor_ended: AtomicBool,
    returned: AtomicBool,
    shared: Mutex<ToolTaskData>,
    input: mpsc::Sender<Value>,
    changed: Notify,
    cache_bytes: usize,
    input_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ToolTaskHandle {
    inner: Arc<ToolTaskShared>,
}

impl ToolTaskHandle {
    pub fn new(limits: &ToolExecutionLimits) -> Result<(Self, mpsc::Receiver<Value>), ToolError> {
        limits.validate()?;
        let (input, receiver) = mpsc::channel(limits.input_capacity);
        Ok((
            Self {
                inner: Arc::new(ToolTaskShared {
                    cancel: CancellationToken::new(),
                    task: Mutex::new(None),
                    task_attached: AtomicBool::new(false),
                    supervisor_ended: AtomicBool::new(false),
                    returned: AtomicBool::new(false),
                    shared: Mutex::new(ToolTaskData {
                        output: Vec::new(),
                        bytes: 0,
                        cursor: 0,
                        replaced: false,
                        completed: false,
                        error: None,
                    }),
                    input,
                    changed: Notify::new(),
                    cache_bytes: limits.cache_bytes,
                    input_bytes: limits.argument_bytes,
                }),
            },
            receiver,
        ))
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

    pub fn claim_return(&self) -> Result<(), ToolError> {
        let state = lock(&self.inner.shared);
        if state.completed {
            return Err(ToolError::new(
                ToolErrorKind::Closed,
                "tool task has completed",
            ));
        }
        if self.inner.returned.swap(true, Ordering::AcqRel) {
            return Err(ToolError::new(
                ToolErrorKind::AlreadyReturned,
                "tool call already returned",
            ));
        }
        Ok(())
    }

    pub fn has_returned(&self) -> bool {
        self.inner.returned.load(Ordering::Acquire)
    }

    pub fn write(&self, value: Value) -> Result<(), ToolError> {
        let bytes = encoded_len(&value, self.inner.cache_bytes)?;
        let mut state = lock(&self.inner.shared);
        ensure_open(&state)?;
        let total = state
            .bytes
            .checked_add(bytes)
            .filter(|&total| total <= self.inner.cache_bytes)
            .ok_or_else(|| {
                ToolError::new(ToolErrorKind::OutputTooLarge, "tool cache exceeds limit")
            })?;
        state.output.push(value);
        state.bytes = total;
        Ok(())
    }

    pub fn replace(&self, value: Value) -> Result<(), ToolError> {
        let bytes = encoded_len(&value, self.inner.cache_bytes)?;
        let mut state = lock(&self.inner.shared);
        ensure_open(&state)?;
        state.output = vec![value];
        state.bytes = bytes;
        state.cursor = 0;
        state.replaced = true;
        Ok(())
    }

    pub fn read(&self) -> TaskReadResult {
        let mut state = lock(&self.inner.shared);
        let result = TaskReadResult {
            replaced: state.replaced,
            output: state.output[state.cursor..].to_vec(),
            completed: state.completed,
            error: state.error.clone(),
        };
        state.cursor = state.output.len();
        state.replaced = false;
        result
    }

    pub fn send(&self, value: Value) -> Result<(), ToolError> {
        let state = lock(&self.inner.shared);
        if state.completed {
            return Err(ToolError::new(
                ToolErrorKind::InputClosed,
                "tool task has completed",
            ));
        }
        encoded_len(&value, self.inner.input_bytes)?;
        self.inner
            .input
            .try_send(value)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    ToolError::new(ToolErrorKind::InputFull, "tool input queue is full")
                }
                mpsc::error::TrySendError::Closed(_) => {
                    ToolError::new(ToolErrorKind::InputClosed, "tool input is closed")
                }
            })
    }

    pub fn set_error(&self, error: ToolError) {
        lock(&self.inner.shared).error = Some(error);
        self.inner.changed.notify_waiters();
    }

    pub fn complete(&self, error: Option<ToolError>) {
        let mut state = lock(&self.inner.shared);
        if state.completed {
            return;
        }
        if let Some(error) = error {
            state.error = Some(error);
        }
        state.completed = true;
        drop(state);
        self.inner.changed.notify_waiters();
    }

    pub fn is_completed(&self) -> bool {
        lock(&self.inner.shared).completed
    }

    pub async fn finish(&self) -> Result<(), ToolError> {
        self.cancel();
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_completed() {
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

fn ensure_open(state: &ToolTaskData) -> Result<(), ToolError> {
    if state.completed {
        Err(ToolError::new(
            ToolErrorKind::Closed,
            "tool task has completed",
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
