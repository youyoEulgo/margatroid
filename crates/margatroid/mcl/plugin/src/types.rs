use crate::MclImportConfig;
use app_runtime_plugin::RuntimeEventSender;
use closure_plugin::WorldClosureSender;
use mcl_core::{MclCommand, MclOperation};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;

pub(crate) struct CommandContext {
    pub request: MclCommand,
    pub operation: MclOperation,
    pub cancel: CancellationToken,
    pub events: RuntimeEventSender,
    pub world: WorldClosureSender,
    pub instance: Option<String>,
    pub turn: Option<String>,
    pub prompts: BTreeMap<String, PathBuf>,
    pub imports: MclImportConfig,
    pub completion: CommandCompletion,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) struct CommandCompletion {
    pub request: MclCommand,
    pub active: std::sync::Arc<Mutex<BTreeMap<String, mcl_core::MclReply>>>,
    pub locked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub owns_lock: bool,
    pub drain: mcl_core::MclDrain,
    pub world: WorldClosureSender,
    pub settled: bool,
}
impl CommandCompletion {
    pub fn finish(&mut self, result: Result<mcl_core::MclValue, mcl_core::MclError>) {
        if self.settled {
            return;
        }
        self.settled = true;
        let cleanup = result
            .as_ref()
            .err()
            .filter(|e| {
                matches!(
                    e.kind,
                    mcl_core::MclErrorKind::CleanupFailed | mcl_core::MclErrorKind::CommitUnknown
                )
            })
            .cloned();
        self.active.lock().unwrap().remove(&self.request.id);
        if self.owns_lock {
            self.locked
                .store(false, std::sync::atomic::Ordering::Release);
        }
        super::handler::respond(&self.request, result);
        self.drain.completed(cleanup);
        if self.owns_lock {
            self.world.send(|_| {});
        }
    }
}
impl Drop for CommandCompletion {
    fn drop(&mut self) {
        if !self.settled {
            self.finish(Err(mcl_core::MclError::new(
                mcl_core::MclErrorKind::CleanupFailed,
                "MCL worker ended without confirmed completion",
            )));
        }
    }
}
