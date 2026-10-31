use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Starting,
    Idle,
    Working,
    Restarting,
    Stopping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStartParameters {
    pub driver: String,
    pub project_root: PathBuf,
    pub home_root: PathBuf,
    pub database_path: PathBuf,
    pub settings_path: PathBuf,
}
