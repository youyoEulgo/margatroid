use crate::{ToolError, ToolErrorKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolExecutionLimits {
    pub script_bytes: usize,
    pub argument_bytes: usize,
    pub output_bytes: usize,
    pub diagnostic_bytes: usize,
    pub memory_bytes: usize,
    pub instruction_budget: u64,
    pub execution_timeout: Duration,
    pub host_call_timeout: Duration,
    pub cleanup_timeout: Duration,
}
impl Default for ToolExecutionLimits {
    fn default() -> Self {
        Self {
            script_bytes: 1024 * 1024,
            argument_bytes: 256 * 1024,
            output_bytes: 1024 * 1024,
            diagnostic_bytes: 256 * 1024,
            memory_bytes: 64 * 1024 * 1024,
            instruction_budget: 10_000_000,
            execution_timeout: Duration::from_secs(30),
            host_call_timeout: Duration::from_secs(10),
            cleanup_timeout: Duration::from_secs(3),
        }
    }
}
impl ToolExecutionLimits {
    pub fn validate(&self) -> Result<(), ToolError> {
        if [
            self.script_bytes,
            self.argument_bytes,
            self.output_bytes,
            self.diagnostic_bytes,
            self.memory_bytes,
        ]
        .contains(&0)
            || self.instruction_budget == 0
            || self.execution_timeout.is_zero()
            || self.host_call_timeout.is_zero()
            || self.cleanup_timeout.is_zero()
            || self.host_call_timeout > self.execution_timeout
        {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "invalid execution limits",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolContext {
    pub agent_id: String,
    pub resource_id: String,
    pub call_id: String,
    pub turn: Option<String>,
    pub project_root: PathBuf,
    pub temp_dir: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[doc(hidden)]
pub struct RunnerRequest {
    pub protocol_version: u32,
    pub script: String,
    pub arguments: serde_json::Value,
    pub context: ToolContext,
    pub limits: ToolExecutionLimits,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[doc(hidden)]
pub struct RunnerResponse {
    pub protocol_version: u32,
    pub result: Result<String, ToolError>,
    pub cleanup_proven: bool,
}
