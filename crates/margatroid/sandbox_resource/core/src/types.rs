use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct SandboxInput {
    pub policy: serde_json::Value,
    pub origin: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SandboxReadLimits {
    pub policy_bytes: usize,
}

impl Default for SandboxReadLimits {
    fn default() -> Self {
        Self {
            policy_bytes: 8 * 1024 * 1024,
        }
    }
}
