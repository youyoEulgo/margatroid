use crate::MclImportConfig;
use app_runtime_plugin::RuntimeEventSender;
use async_runtime_plugin::AsyncTaskError;
use closure_plugin::WorldClosureSender;
use mcl_core::{MclCommand, MclError, MclErrorKind, MclOperation, MclValue};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;

pub(crate) struct CommandContext {
    pub request: MclCommand,
    pub history_key: String,
    pub operation: MclOperation,
    pub cancel: CancellationToken,
    pub events: RuntimeEventSender,
    pub world: WorldClosureSender,
    pub instance: Option<String>,
    pub turn: Option<String>,
    pub prompts: BTreeMap<String, PathBuf>,
    pub imports: MclImportConfig,
    pub allowed_imports: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CommandOutput {
    pub request: MclCommand,
    pub value: MclValue,
}

#[derive(Clone, Debug)]
pub(crate) struct CommandError {
    pub request: Option<MclCommand>,
    pub error: MclError,
}

impl From<AsyncTaskError> for CommandError {
    fn from(error: AsyncTaskError) -> Self {
        Self {
            request: None,
            error: MclError::new(MclErrorKind::CleanupFailed, error.to_string()),
        }
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
