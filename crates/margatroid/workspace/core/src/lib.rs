mod components;
mod database;
mod error;
mod events;
mod pending;
mod sync;
mod types;

pub use components::Workspace;
pub use database::{WorkspaceDatabase, WorkspaceOperation};
pub use error::{WorkspaceError, WorkspaceErrorKind};
pub use events::*;
pub use pending::{WorkspacePendingWork, WorkspaceWorkToken};
pub use sync::WorkspaceUpdateContext;
pub use types::*;

use core_plugin::Resource;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
}
impl Resource for WorkspaceConfig {}
