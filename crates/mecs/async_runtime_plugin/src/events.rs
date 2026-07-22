#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AsyncTaskId(pub(crate) u64);

impl AsyncTaskId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncTaskFailureKind {
    QueueFull,
    WorkerStopped,
    Timeout,
    Cancelled,
    Panic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsyncTaskStarted {
    pub task_id: AsyncTaskId,
    pub request_type: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsyncTaskFailed {
    pub task_id: AsyncTaskId,
    pub request_type: &'static str,
    pub kind: AsyncTaskFailureKind,
    pub message: String,
}
