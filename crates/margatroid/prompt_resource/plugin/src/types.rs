use std::path::PathBuf;

use core_plugin::{Entity, Resource};
use prompt_resource_core::{ImportPromptReply, PromptReadLimits, PromptResourceError};
use tokio_util::sync::CancellationToken;

pub(crate) struct ImportConfig {
    pub limits: PromptReadLimits,
}

impl Resource for ImportConfig {}

#[derive(Clone)]
pub(crate) struct ImportContext {
    pub owner: Entity,
    pub alias: String,
    pub path: PathBuf,
    pub cancel: CancellationToken,
    pub limits: PromptReadLimits,
    pub reply: ImportPromptReply,
}

#[derive(Clone)]
pub(crate) struct ImportOutput {
    pub context: ImportContext,
    pub path: PathBuf,
}

#[derive(Clone)]
pub(crate) struct ImportError {
    pub context: ImportContext,
    pub error: PromptResourceError,
}
