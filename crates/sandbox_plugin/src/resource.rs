use sandbox::config::SandboxConfig;

#[derive(Clone, Debug, Default)]
pub struct SandboxPolicy {
    pub config: SandboxConfig,
}

impl SandboxPolicy {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SandboxExecutor;

impl SandboxExecutor {
    pub fn new() -> Self {
        Self
    }
}
