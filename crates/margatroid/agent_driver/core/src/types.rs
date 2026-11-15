use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentDriverInput {
    pub program: String,
    pub origin: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDriverReadLimits {
    pub program_bytes: usize,
    pub settings_bytes: usize,
}

impl Default for AgentDriverReadLimits {
    fn default() -> Self {
        Self {
            program_bytes: 4 * 1024 * 1024,
            settings_bytes: 256 * 1024,
        }
    }
}
