use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::thread::JoinHandle;

use core_plugin::Resource;
use tokio::sync::mpsc::UnboundedSender;

use crate::AsyncRuntimeError;

pub(crate) type ErasedExecutionTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub struct AsyncRuntimeHandle {
    sender: Option<UnboundedSender<ErasedExecutionTask>>,
    thread: Option<JoinHandle<()>>,
}

impl AsyncRuntimeHandle {
    pub(crate) fn new(
        sender: UnboundedSender<ErasedExecutionTask>,
        thread: JoinHandle<()>,
    ) -> Self {
        Self {
            sender: Some(sender),
            thread: Some(thread),
        }
    }

    /// Submit an owned future without projecting its completion into an ECS event.
    /// The caller retains responsibility for replies, cancellation and cleanup.
    pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.sender
            .as_ref()
            .expect("async executor sender must exist before drop")
            .send(Box::pin(task))
            .unwrap_or_else(|_| AsyncRuntimeError::ExecutorDisconnected.panic());
    }
}

impl Drop for AsyncRuntimeHandle {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            thread.join().expect("async executor thread panicked");
        }
    }
}

impl Resource for AsyncRuntimeHandle {}

pub(crate) struct AsyncRegistry {
    event_systems: HashMap<TypeId, &'static str>,
}

impl AsyncRegistry {
    pub(crate) fn new() -> Self {
        Self {
            event_systems: HashMap::new(),
        }
    }

    pub(crate) fn register_event_system<Request: core_plugin::Event>(&mut self) -> bool {
        self.event_systems
            .insert(TypeId::of::<Request>(), std::any::type_name::<Request>())
            .is_none()
    }

    pub(crate) fn contains_event_system<Request: core_plugin::Event>(&self) -> bool {
        self.event_systems.contains_key(&TypeId::of::<Request>())
    }
}

impl Resource for AsyncRegistry {}
