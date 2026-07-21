use tokio::process::Command;

use crate::events::{
    SandboxCommandCompleted, SandboxCommandFailed, SandboxCommandRequested, SandboxFailureKind,
};
use crate::resource::SandboxPolicy;

#[derive(Clone, Debug)]
pub enum SandboxAsyncOutput {
    Completed(SandboxCommandCompleted),
    Failed(SandboxCommandFailed),
}

pub(crate) async fn execute_sandbox_command(
    policy: SandboxPolicy,
    request: SandboxCommandRequested,
) -> SandboxAsyncOutput {
    let command = if request.use_sandbox && policy.config.enabled {
        let mut manager = sandbox::SandboxManager::new();
        if let Err(error) = manager.initialize(policy.config).await {
            return SandboxAsyncOutput::Failed(SandboxCommandFailed {
                command_id: request.command_id,
                kind: SandboxFailureKind::SpawnFailed,
                message: error.to_string(),
            });
        }
        let wrapped = manager.wrap_command(&request.command);
        if let Err(error) = manager.guard(&wrapped) {
            return SandboxAsyncOutput::Failed(SandboxCommandFailed {
                command_id: request.command_id,
                kind: SandboxFailureKind::PermissionDenied,
                message: error.to_string(),
            });
        }
        wrapped
    } else {
        request.command
    };

    let mut child = Command::new("sh");
    child.arg("-lc").arg(command);
    if let Some(current_dir) = request.current_dir {
        child.current_dir(current_dir);
    }

    match child.output().await {
        Ok(output) if output.status.success() => {
            SandboxAsyncOutput::Completed(SandboxCommandCompleted {
                command_id: request.command_id,
                status: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
        Ok(output) => SandboxAsyncOutput::Failed(SandboxCommandFailed {
            command_id: request.command_id,
            kind: SandboxFailureKind::ExitNonZero,
            message: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Err(error) => SandboxAsyncOutput::Failed(SandboxCommandFailed {
            command_id: request.command_id,
            kind: SandboxFailureKind::SpawnFailed,
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_runtime_plugin::AsyncRuntimePlugin;
    use core_plugin::{App, Stage, World};

    use crate::{SandboxCommandCompleted, SandboxCommandRequested, SandboxPlugin};

    #[test]
    fn plugin_executes_command_without_sandbox() {
        let mut app = App::new();
        app.add_plugins(AsyncRuntimePlugin::default());
        app.add_plugins(SandboxPlugin::new());

        let completed = Arc::new(Mutex::new(Vec::new()));
        let system_completed = completed.clone();
        let mut reader = app.event_reader::<SandboxCommandCompleted>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                system_completed
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );

        app.world()
            .send_event(SandboxCommandRequested::new("cmd-1", "printf hello").without_sandbox());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while completed.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let completed = completed.lock().unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].stdout, "hello");
    }
}
