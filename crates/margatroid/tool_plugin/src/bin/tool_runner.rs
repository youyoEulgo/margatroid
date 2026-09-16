use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use margatroid_types::ResourceId;
use serde::Deserialize;
use tool_plugin::{run_lua_tool, LuaExecutionLimits, LuaToolRunRequest};

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

fn usage(message: &str) -> ExitCode {
    eprintln!("tool_runner: {message}");
    eprintln!("usage: tool_runner <request.json>");
    ExitCode::from(2)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1) else {
        return usage("missing request file");
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => return usage(&format!("cannot read request: {error}")),
    };
    let request = match serde_json::from_str::<RunnerRequest>(&raw) {
        Ok(request) => request,
        Err(error) => return usage(&format!("invalid request: {error}")),
    };
    let (Ok(agent_id), Ok(resource_id)) = (
        ResourceId::parse(&request.agent_id),
        ResourceId::parse(&request.resource_id),
    ) else {
        return usage("invalid resource id");
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
            print!("{result}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("tool_runner: {error}");
            ExitCode::from(3)
        }
    }
}
