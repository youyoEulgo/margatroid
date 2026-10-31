use config_plugin::ConfigPlugin;
use inference_plugin::InferencePlugin;
use std::path::PathBuf;
use workspace_plugin::WorkspacePlugin;
#[derive(Clone, Debug)]
pub struct ApplicationOptions {
    pub data_root: PathBuf,
    pub runner_path: PathBuf,
}
pub(crate) struct ApplicationParts {
    pub options: ApplicationOptions,
    pub config: ConfigPlugin,
    pub inference: InferencePlugin,
    pub workspace: WorkspacePlugin,
}
