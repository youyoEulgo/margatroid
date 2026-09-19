use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_plugin::{Agent, AgentResourceMap};
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_types::ResourceId;

use crate::{ResourceContent, ResourceMapEntry, ToolError, ToolErrorKind, ToolRegisterRequest};

const SANDBOX_TYPE: &str = "sandbox";
const POLICY_FILE: &str = "policy.json";
const MAX_POLICY_BYTES: usize = 1024 * 1024;
static POLICY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Resolve the effective policies for a tool invocation.
///
/// Every activated sandbox contributes its policy, in a deterministic order;
/// landstrip merges them so the result can only narrow. An activated sandbox
/// without a usable policy is a hard error rather than a silent downgrade.
pub(crate) fn active_sandbox_policies(
    resources: &AgentResourceMap,
) -> Result<Vec<Arc<str>>, ToolError> {
    resources
        .active_sandboxes
        .iter()
        .map(|resource_id| {
            let policy = resources.sandbox_policies.get(resource_id).ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    format!("active sandbox {resource_id} has no policy"),
                )
            })?;
            if policy.trim().is_empty() {
                return Err(ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    format!("active sandbox {resource_id} has an empty policy"),
                ));
            }
            tracing::debug!(sandbox = %resource_id, "resolved active sandbox policy");
            Ok(Arc::clone(policy))
        })
        .collect()
}

pub(crate) fn sandbox_backend() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MARGATROID_SANDBOX_BACKEND") {
        return Some(PathBuf::from(path));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join("landstrip");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let directory = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidate = directory.join("landstrip");
    candidate.is_file().then_some(candidate)
}

/// Materialise an active policy for a single confined spawn.
///
/// Each invocation gets its own file because the policy is per-agent state and
/// never a process-wide constant.
pub(crate) fn write_policy_file(policy: &str) -> Result<PathBuf, ToolError> {
    let path = std::env::temp_dir().join(format!(
        "margatroid-sandbox-policy-{}-{}.json",
        std::process::id(),
        POLICY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, policy.as_bytes()).map_err(|error| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            format!("sandbox policy could not be written: {error}"),
        )
    })?;
    Ok(path)
}

/// Wrap `argv` so the spawn is confined by every active policy.
///
/// A declared policy with no usable backend fails closed: the tool is refused
/// instead of silently running outside the policy the driver selected.
pub(crate) fn confined_command(
    policies: &[PathBuf],
    program: &Path,
) -> Result<tokio::process::Command, ToolError> {
    if policies.is_empty() {
        return Ok(tokio::process::Command::new(program));
    }
    let backend = sandbox_backend().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            "a sandbox policy is active but the landstrip backend was not found; install landstrip",
        )
    })?;
    let mut command = tokio::process::Command::new(backend);
    command.arg("run");
    for policy in policies {
        command.arg("-p").arg(policy);
    }
    command.arg("--").arg(program);
    Ok(command)
}

/// Remove the per-invocation policy files once the confined spawn is done.
pub(crate) fn remove_policy_files(policies: &[PathBuf]) {
    for policy in policies {
        let _ = fs::remove_file(policy);
    }
}

pub(crate) fn sandbox_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolRegisterRequest>()
        .into_iter()
        .filter(|request| request.resource_id.resource_type() == SANDBOX_TYPE)
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let result = register_sandbox_resource(world, &request);
        if let Err(error) = &result {
            tracing::error!(resource = %request.resource_id, error = %error, "sandbox resource registration failed");
        }
        world.send_event(crate::ToolRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            alias: request.alias,
            result,
        });
    }
}

fn register_sandbox_resource(
    world: &World,
    request: &ToolRegisterRequest,
) -> Result<ResourceMapEntry, ToolError> {
    if request.id.is_empty() || !world.is_alive(request.agent) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "sandbox registration request is invalid",
        ));
    }
    let agent = world.get_component::<Agent>(request.agent).ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::ToolEnvironmentMissing,
            "agent tool environment is missing",
        )
    })?;
    let root = find_sandbox_package(
        &agent.info.project_root,
        &agent.info.image_root,
        &request.resource_id,
    )?;
    let policy = fs::read_to_string(root.join(POLICY_FILE)).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "sandbox policy could not be read",
        )
    })?;
    if policy.is_empty() || policy.len() > MAX_POLICY_BYTES {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "sandbox policy is empty or too large",
        ));
    }
    serde_json::from_str::<serde_json::Value>(&policy).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "sandbox policy must be valid JSON",
        )
    })?;
    Ok(ResourceMapEntry {
        resource_id: request.resource_id.clone(),
        resource_name: request
            .alias
            .clone()
            .unwrap_or_else(|| request.resource_id.to_string()),
        alias: request.alias.clone(),
        tool_id: None,
        template: None,
        content: Some(ResourceContent::Sandbox {
            policy: Arc::from(policy),
        }),
    })
}

fn find_sandbox_package(
    project_root: &Path,
    image_root: &Path,
    resource_id: &ResourceId,
) -> Result<PathBuf, ToolError> {
    if resource_id.resource_type() != SANDBOX_TYPE {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "sandbox resource must use type sandbox",
        ));
    }
    let roots = [
        project_root.join(".margatroid").join("sandboxes"),
        image_root.join("sandboxes"),
    ];
    for root in roots {
        let package = root
            .join(resource_id.scope())
            .join(resource_id.name())
            .join(resource_id.tag());
        if package.join(POLICY_FILE).is_file() {
            return Ok(package);
        }
    }
    Err(ToolError::new(
        ToolErrorKind::ResourceResolutionFailed,
        "sandbox policy was not found",
    ))
}
