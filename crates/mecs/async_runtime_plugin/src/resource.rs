use std::any::TypeId;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::thread::JoinHandle;

use core_plugin::Resource;
use tokio::sync::mpsc::UnboundedSender;

use crate::{AsyncRequest, AsyncRuntimeError};

pub(crate) type ErasedExecutionTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub(crate) struct AsyncExecutorHandle {
    sender: Option<UnboundedSender<ErasedExecutionTask>>,
    thread: Option<JoinHandle<()>>,
}

impl AsyncExecutorHandle {
    pub(crate) fn new(
        sender: UnboundedSender<ErasedExecutionTask>,
        thread: JoinHandle<()>,
    ) -> Self {
        Self {
            sender: Some(sender),
            thread: Some(thread),
        }
    }

    pub(crate) fn spawn(&self, task: ErasedExecutionTask) {
        self.sender
            .as_ref()
            .expect("async executor sender must exist before drop")
            .send(task)
            .unwrap_or_else(|_| AsyncRuntimeError::ExecutorDisconnected.panic());
    }
}

impl Drop for AsyncExecutorHandle {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            thread.join().expect("async executor thread panicked");
        }
    }
}

impl Resource for AsyncExecutorHandle {}

pub(crate) struct AsyncRequestRegistry {
    registered: HashSet<TypeId>,
}

impl AsyncRequestRegistry {
    pub(crate) fn new() -> Self {
        Self {
            registered: HashSet::new(),
        }
    }

    pub(crate) fn register<T, E>(&mut self) -> bool
    where
        T: Send + Sync + 'static,
        E: From<crate::AsyncTaskError> + Send + Sync + 'static,
    {
        self.registered.insert(TypeId::of::<AsyncRequest<T, E>>())
    }
}

impl Resource for AsyncRequestRegistry {}
