use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use margatroid_types::ResourceId;
use serde::Deserialize;
use tool_plugin::{run_lua_tool, LuaExecutionLimits, LuaToolRunRequest, ToolError, ToolErrorKind};

#[derive(Deserialize)]
struct RunnerRequest {
    package_root: PathBuf,
    arguments: String,
    agent_id: String,
    turn_id: String,
    resource_id: String,
    project_root: PathBuf,
    image_root: PathBuf,
    limits: RunnerLimits,
}

#[derive(Deserialize)]
struct RunnerLimits {
    max_definition_bytes: usize,
    max_script_bytes: usize,
    max_argument_bytes: usize,
    max_output_bytes: usize,
    max_memory_bytes: usize,
    max_instructions: u64,
    max_execution_time_ms: u64,
    max_host_call_time_ms: u64,
}

fn kind_name(kind: ToolErrorKind) -> &'static str {
    match kind {
        ToolErrorKind::InvalidRequest => "InvalidRequest",
        ToolErrorKind::InvalidArguments => "InvalidArguments",
        ToolErrorKind::InvalidDefinition => "InvalidDefinition",
        ToolErrorKind::RunnerFailed => "RunnerFailed",
        _ => "ExecutionFailed",
    }
}

fn report(error: &ToolError) -> ExitCode {
    let payload = serde_json::json!({
        "kind": kind_name(error.kind()),
        "message": error.message(),
    });
    eprintln!("{payload}");
    match error.kind() {
        ToolErrorKind::InvalidRequest
        | ToolErrorKind::InvalidArguments
        | ToolErrorKind::InvalidDefinition => ExitCode::from(2),
        _ => ExitCode::from(3),
    }
}

fn request_payload() -> Result<String, String> {
    if let Some(path) = std::env::args_os().nth(1) {
        return std::fs::read_to_string(&path).map_err(|error| format!("cannot read request: {error}"));
    }
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .map_err(|error| format!("cannot read request from stdin: {error}"))?;
    Ok(raw)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let raw = match request_payload() {
        Ok(raw) => raw,
        Err(message) => {
            eprintln!("{}", serde_json::json!({ "kind": "RunnerFailed", "message": message }));
            return ExitCode::from(2);
        }
    };
    let request = match serde_json::from_str::<RunnerRequest>(&raw) {
        Ok(request) => request,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({ "kind": "RunnerFailed", "message": format!("invalid request: {error}") })
            );
            return ExitCode::from(2);
        }
    };
    let (Ok(agent_id), Ok(resource_id)) = (
        ResourceId::parse(&request.agent_id),
        ResourceId::parse(&request.resource_id),
    ) else {
        eprintln!(
            "{}",
            serde_json::json!({ "kind": "RunnerFailed", "message": "invalid resource id" })
        );
        return ExitCode::from(2);
    };
    let limits = LuaExecutionLimits {
        max_definition_bytes: request.limits.max_definition_bytes,
        max_script_bytes: request.limits.max_script_bytes,
        max_argument_bytes: request.limits.max_argument_bytes,
        max_output_bytes: request.limits.max_output_bytes,
        max_memory_bytes: request.limits.max_memory_bytes,
        max_instructions: request.limits.max_instructions,
        max_execution_time: Duration::from_millis(request.limits.max_execution_time_ms),
        max_host_call_time: Duration::from_millis(request.limits.max_host_call_time_ms),
    };
    let outcome = run_lua_tool(LuaToolRunRequest {
        package_root: request.package_root,
        arguments: request.arguments,
        agent_id,
        turn_id: request.turn_id,
        resource_id,
        project_root: request.project_root,
        image_root: request.image_root,
        limits,
        client: None,
    })
    .await;
    match outcome {
        Ok(result) => {
            let _ = std::io::stdout().write_all(result.as_bytes());
            ExitCode::SUCCESS
        }
        Err(error) => report(&error),
    }
}
