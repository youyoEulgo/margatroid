use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Starting,
    Running,
    Error,
    Stopping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStartParameters {
    pub initial_settings_path: PathBuf,
    pub ui_path: PathBuf,
    pub project_root: PathBuf,
    pub home_root: PathBuf,
    pub database_path: PathBuf,
    pub settings_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct AgentResourceRoot(pub PathBuf);
impl core_plugin::Resource for AgentResourceRoot {}
