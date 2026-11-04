use std::path::PathBuf;

use core_plugin::{Entity, Resource};
use tokio_util::sync::CancellationToken;
use tool_resource_core::{ImportToolReply, ToolReadLimits, ToolResourceError};

pub(crate) struct ImportConfig {
    pub schedule: String,
    pub limits: ToolReadLimits,
}

impl Resource for ImportConfig {}

#[derive(Clone)]
pub(crate) struct ImportContext {
    pub owner: Entity,
    pub alias: String,
    pub path: PathBuf,
    pub cancel: CancellationToken,
    pub limits: ToolReadLimits,
    pub reply: ImportToolReply,
}

#[derive(Clone)]
pub(crate) struct ImportOutput {
    pub context: ImportContext,
    pub path: PathBuf,
}

#[derive(Clone)]
pub(crate) struct ImportError {
    pub context: ImportContext,
    pub error: ToolResourceError,
}

/// Infrastructure failures are covered by the closure's owned reply guard.
#[derive(Clone)]
pub(crate) enum ImportTaskError {
    Validation(ImportError),
    Runtime,
}
impl From<async_runtime_plugin::AsyncTaskError> for ImportTaskError {
    fn from(_: async_runtime_plugin::AsyncTaskError) -> Self {
        Self::Runtime
    }
}
