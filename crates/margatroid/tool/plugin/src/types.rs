use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tool_core::{
    ToolConfiguration, ToolError, ToolExecuteRequest, ToolExecuteResponse, ToolExecutionLimits,
    ToolTaskHandle, ToolTasks,
};

pub(crate) struct ToolExecutionContext {
    pub request: ToolExecuteRequest,
    pub reply: Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>,
    pub tool_path: PathBuf,
    pub sandbox_path: Option<PathBuf>,
    pub project_root: PathBuf,
    pub runner_path: PathBuf,
    pub limits: ToolExecutionLimits,
    pub configuration: Option<ToolConfiguration>,
    pub handle: ToolTaskHandle,
    pub tasks: ToolTasks,
    pub input: mpsc::Receiver<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RunnerRequest {
    pub protocol_version: u32,
    pub program: String,
    pub arguments: Value,
    pub context: ToolContext,
    pub limits: ToolExecutionLimits,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ToolContext {
    pub task_id: String,
    pub call_id: String,
    pub alias: String,
    pub project_root: PathBuf,
    pub temp_dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RunnerWire {
    pub protocol_version: u32,
    pub frame: RunnerFrame,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum RunnerFrame {
    Configured {
        configuration: ToolConfiguration,
    },
    Return {
        request_id: String,
        value: Value,
    },
    Write {
        request_id: String,
        value: Value,
    },
    Replace {
        request_id: String,
        value: Value,
    },
    Read {
        request_id: String,
        task_id: String,
    },
    Send {
        request_id: String,
        task_id: String,
        value: Value,
    },
    Finish {
        request_id: String,
        task_id: String,
    },
    InputConsumed,
    Done {
        result: Result<(), ToolError>,
    },
    Stopped {
        result: Result<(), ToolError>,
        cleanup_proven: bool,
    },
}

#[derive(Serialize, Deserialize)]
pub(crate) struct HostWire {
    pub protocol_version: u32,
    pub frame: HostFrame,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum HostFrame {
    Configured,
    Input {
        value: Value,
    },
    Response {
        request_id: String,
        result: Result<Value, ToolError>,
    },
}

pub(crate) type PendingResponse = oneshot::Sender<Result<Value, ToolError>>;

pub(crate) struct WorkerBridge {
    pub output: tokio::sync::Mutex<tokio::io::Stdout>,
    pub pending: Mutex<BTreeMap<String, PendingResponse>>,
    pub input: tokio::sync::Mutex<mpsc::Receiver<Value>>,
    pub sequence: AtomicU64,
    pub frame_limit: usize,
    pub finish: CancellationToken,
}

pub(crate) struct CgroupBoundary {
    pub path: PathBuf,
}

pub(crate) struct NoExternalSchema;

pub(crate) type SharedBridge = Arc<WorkerBridge>;

#[cfg(target_os = "linux")]
#[derive(Clone)]
pub(crate) struct TerminalSession {
    pub(crate) inner: std::sync::Arc<TerminalInner>,
}
#[cfg(target_os = "linux")]
pub(crate) struct TerminalInner {
    pub(crate) master: tokio::io::unix::AsyncFd<std::fs::File>,
    pub(crate) child: tokio::sync::Mutex<tokio::process::Child>,
    pub(crate) output: std::sync::Mutex<TerminalOutput>,
}

#[cfg(target_os = "linux")]
pub(crate) struct TerminalOutput {
    pub(crate) parser: vt100::Parser,
    pub(crate) history: String,
    pub(crate) previous: (String, bool),
    pub(crate) limit: usize,
}

#[derive(Clone)]
pub(crate) struct ToolLaunchOutput;
#[derive(Clone)]
pub(crate) struct ToolLaunchError {
    pub fallback: Option<crate::handler::ToolLaunchFallback>,
    pub error: ToolError,
}
impl From<async_runtime_plugin::AsyncTaskError> for ToolLaunchError {
    fn from(error: async_runtime_plugin::AsyncTaskError) -> Self {
        Self {
            fallback: None,
            error: ToolError::new(tool_core::ToolErrorKind::SubmitFailed, error.to_string()),
        }
    }
}
