use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Shared launch and join state. An empty launch slot is still active.
#[derive(Default)]
pub struct InferenceTaskState {
    state: std::sync::Mutex<InferenceTaskStatus>,
    changed: tokio::sync::Notify,
}
// Tokio JoinHandle stores a single waker. Broadcast its completion to every
// registered waiter so dropping the last waiter cannot strand the others.
struct InferenceTaskWake(std::sync::Weak<InferenceTaskState>);
impl std::task::Wake for InferenceTaskWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(state) = self.0.upgrade() {
            state.changed.notify_waiters();
        }
    }
}
#[derive(Default)]
enum InferenceTaskStatus {
    #[default]
    Pending,
    Running(JoinHandle<()>),
    Finished(Result<(), crate::InferenceError>),
}
#[derive(Clone)]
pub struct InferenceHandle {
    pub request_id: String,
    pub turn: String,
    pub cancel: CancellationToken,
    pub stop_stream: Arc<dyn Fn() + Send + Sync>,
    pub task: Arc<InferenceTaskState>,
}
impl InferenceHandle {
    pub fn attach_task(&self, task: JoinHandle<()>) -> Result<(), crate::InferenceError> {
        let mut state = self
            .task
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !matches!(*state, InferenceTaskStatus::Pending) {
            return Err(crate::InferenceError::new(
                crate::InferenceErrorKind::TaskPanicked,
                "inference task already attached or launch failed",
            ));
        }
        *state = InferenceTaskStatus::Running(task);
        drop(state);
        self.task.changed.notify_waiters();
        Ok(())
    }
    pub fn fail_launch(&self, error: crate::InferenceError) {
        let mut state = self
            .task
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if matches!(*state, InferenceTaskStatus::Pending) {
            *state = InferenceTaskStatus::Finished(Err(error));
        }
        drop(state);
        self.task.changed.notify_waiters();
    }
    pub fn cancel(&self) {
        (self.stop_stream)();
        self.cancel.cancel();
    }
    pub fn is_finished(&self) -> bool {
        match &*self
            .task
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
        {
            InferenceTaskStatus::Pending => false,
            InferenceTaskStatus::Running(task) => task.is_finished(),
            InferenceTaskStatus::Finished(_) => true,
        }
    }
    pub async fn wait_finished(&self) -> Result<(), crate::InferenceError> {
        use std::future::Future;
        use std::task::Poll;
        loop {
            let changed = self.task.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let result = std::future::poll_fn(|cx| {
                let mut state = self
                    .task
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                match &mut *state {
                    InferenceTaskStatus::Pending => {}
                    InferenceTaskStatus::Running(task) => {
                        let waker = std::task::Waker::from(Arc::new(InferenceTaskWake(
                            Arc::downgrade(&self.task),
                        )));
                        let mut task_context = std::task::Context::from_waker(&waker);
                        if let Poll::Ready(result) =
                            std::pin::Pin::new(task).poll(&mut task_context)
                        {
                            let result = result.map_err(|_| {
                                crate::InferenceError::new(
                                    crate::InferenceErrorKind::TaskPanicked,
                                    "inference supervisor failed",
                                )
                            });
                            *state = InferenceTaskStatus::Finished(result.clone());
                            drop(state);
                            self.task.changed.notify_waiters();
                            return Poll::Ready(Some(result));
                        }
                    }
                    InferenceTaskStatus::Finished(result) => {
                        return Poll::Ready(Some(result.clone()))
                    }
                }
                if changed.as_mut().poll(cx).is_ready() {
                    Poll::Ready(None)
                } else {
                    Poll::Pending
                }
            })
            .await;
            if let Some(result) = result {
                return result;
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceParameters {
    pub model: String,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stop: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    pub model: String,
    pub provider: String,
    pub api_type: String,
    pub base_url: String,
    pub api_key: String,
    pub context_window_tokens: u64,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}
impl std::fmt::Debug for ModelRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRoute")
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("api_type", &self.api_type)
            .field("context_window_tokens", &self.context_window_tokens)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainRequest {
    pub messages: Vec<InferenceMessage>,
    pub tools: Vec<InferenceToolDefinition>,
    pub config: InferenceParameters,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InferenceMessage {
    System {
        content: String,
    },
    User {
        content: String,
        #[serde(default, rename = "data")]
        images: Vec<InferenceImage>,
    },
    Assistant {
        reasoning: Option<String>,
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<InferenceToolCall>,
    },
    Tool {
        call_id: String,
        content: String,
        #[serde(default, rename = "data")]
        images: Vec<InferenceImage>,
        #[serde(default)]
        failed: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceImage {
    pub media_type: String,
    pub data: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceResponse {
    pub reasoning: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Vec<InferenceToolCall>,
    pub usage: Option<InferenceUsage>,
    pub stop_reason: InferenceStopReason,
}
impl InferenceResponse {
    pub fn empty_completed() -> Self {
        Self {
            reasoning: None,
            content: None,
            tool_calls: vec![],
            usage: None,
            stop_reason: InferenceStopReason::Completed,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceStopReason {
    Completed,
    ToolCalls,
    Length,
    ContentFilter,
    Other(String),
}
impl InferenceStopReason {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Completed => "completed",
            Self::ToolCalls => "tool_calls",
            Self::Length => "length",
            Self::ContentFilter => "content_filter",
            Self::Other(v) => v,
        }
    }
}
impl std::fmt::Display for InferenceStopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
