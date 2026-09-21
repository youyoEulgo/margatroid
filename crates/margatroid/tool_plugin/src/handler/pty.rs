//! The one place a command runs behind a pseudo-terminal.
//!
//! A tool that needs a terminal — an interactive program, or one that branches
//! on `isatty` — cannot be served by `io.popen`, which is a pipe. Both the
//! shell tool and the Lua tool now route through here, so a command behaves the
//! same whichever tool asked for it, and the process-group, timeout and output
//! limits have a single implementation.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use crate::handler::sandbox::{ConfinedSpawn, ProcessGroup};
use crate::{ToolError, ToolErrorKind};

/// The file name a staged interpreter script takes inside the scratch directory.
/// The shell tool and the Lua shell share it because both hand the file to the
/// same runner below.
pub(crate) const SHELL_SCRIPT_FILE: &str = "main.sh";

/// A command and the script that decides how to run it.
#[derive(Clone, Copy)]
pub(crate) struct PtyRequest<'a> {
    pub(crate) program: &'a Path,
    pub(crate) script: &'a Path,
    pub(crate) command: &'a str,
    pub(crate) project_root: &'a Path,
    pub(crate) max_output_bytes: usize,
    pub(crate) max_execution_time: Duration,
}

/// What a finished pty run produced. `exit_code` is absent when the child was
/// killed by a signal, which is how a timeout surfaces here.
#[derive(Debug)]
pub(crate) struct PtyOutcome {
    pub(crate) exit_code: Option<i32>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

pub(crate) async fn run_in_pty(
    request: PtyRequest<'_>,
    spawn: &ConfinedSpawn,
) -> Result<PtyOutcome, ToolError> {
    let (master, mut child) = spawn_pty(request, spawn)?;
    let mut group = ProcessGroup::confined(spawn.is_confined(), child.id().unwrap_or_default());
    let limit = request.max_output_bytes;
    let mut master = master;

    let read = async {
        let mut output = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 8192];
        loop {
            match master.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => extend_bounded(&mut output, &buffer[..read], limit, &mut truncated),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                // The PTY reports end-of-stream as EIO once the last slave
                // closes, which is the normal way this loop finishes.
                Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => break,
                Err(_) => break,
            }
        }
        (output, truncated)
    };

    let status = tokio::select! {
        status = child.wait() => status,
        _ = tokio::time::sleep(request.max_execution_time) => {
            group.reclaim();
            return Err(ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Shell process timed out",
            ));
        }
    };
    group.reclaim();
    let status = status.map_err(|_| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell process could not be awaited",
        )
    })?;
    let (bytes, truncated) = read.await;
    Ok(PtyOutcome {
        exit_code: status.code(),
        bytes,
        truncated,
    })
}

fn spawn_pty(
    request: PtyRequest<'_>,
    spawn: &ConfinedSpawn,
) -> Result<(tokio::fs::File, tokio::process::Child), ToolError> {
    let pty = nix::pty::openpty(None, None)
        .map_err(|_| ToolError::new(ToolErrorKind::ExecutionFailed, "PTY could not be created"))?;
    let master = tokio::fs::File::from_std(std::fs::File::from(pty.master));
    let slave_fd = std::os::fd::AsRawFd::as_raw_fd(&pty.slave);
    let duplicate = |error: std::io::Error| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            format!("PTY could not be duplicated: {error}"),
        )
    };
    let stdin = Stdio::from(pty.slave.try_clone().map_err(duplicate)?);
    let stdout = Stdio::from(pty.slave.try_clone().map_err(duplicate)?);
    let stderr = Stdio::from(pty.slave);
    let mut process = spawn.command(request.program, request.project_root)?;
    // The terminal type belongs to this path: the pipe path sets TERM=dumb to
    // keep captured output plain, while a real terminal needs a real value.
    process.env("TERM", "xterm-256color");
    process
        .arg(request.script)
        .arg(request.command)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    unsafe {
        process.pre_exec(move || {
            if nix::unistd::setsid().is_err() {
                return Err(std::io::Error::last_os_error());
            }
            if nix::libc::ioctl(slave_fd, nix::libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = process.spawn().map_err(|_| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell process could not be started",
        )
    })?;
    Ok((master, child))
}

fn extend_bounded(output: &mut Vec<u8>, chunk: &[u8], limit: usize, truncated: &mut bool) {
    let remaining = limit.saturating_sub(output.len());
    if remaining > 0 {
        output.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    if chunk.len() > remaining {
        *truncated = true;
    }
}

/// A PTY yields CRLF line endings; callers want the text a pipe would give.
pub(crate) fn normalize_pty_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("margatroid-pty-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    /// A pty gives the child a terminal, which a pipe cannot: `test -t 1`
    /// succeeds inside the sandbox only when a pty was actually allocated.
    #[tokio::test]
    async fn the_child_sees_a_terminal() {
        let directory = scratch("tty");
        let script = directory.join("main.sh");
        std::fs::write(&script, "exec bash -lc \"$1\"\n").expect("script");
        let spawn = ConfinedSpawn::new(&[]).expect("unconfined spawn");
        let outcome = run_in_pty(
            PtyRequest {
                program: Path::new("bash"),
                script: &script,
                command: "test -t 1 && echo tty=yes || echo tty=no",
                project_root: &directory,
                max_output_bytes: 64 * 1024,
                max_execution_time: Duration::from_secs(10),
            },
            &spawn,
        )
        .await
        .expect("pty run");
        let text = normalize_pty_text(&outcome.bytes);
        assert!(text.contains("tty=yes"), "expected a terminal, got: {text}");
        assert_eq!(outcome.exit_code, Some(0));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn the_exit_code_and_output_come_back() {
        let directory = scratch("exit");
        let script = directory.join("main.sh");
        std::fs::write(&script, "exec bash -lc \"$1\"\n").expect("script");
        let spawn = ConfinedSpawn::new(&[]).expect("unconfined spawn");
        let outcome = run_in_pty(
            PtyRequest {
                program: Path::new("bash"),
                script: &script,
                command: "echo captured; exit 3",
                project_root: &directory,
                max_output_bytes: 64 * 1024,
                max_execution_time: Duration::from_secs(10),
            },
            &spawn,
        )
        .await
        .expect("pty run");
        let text = normalize_pty_text(&outcome.bytes);
        assert!(text.contains("captured"), "output missing: {text}");
        assert_eq!(outcome.exit_code, Some(3));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn output_beyond_the_limit_is_truncated() {
        let directory = scratch("limit");
        let script = directory.join("main.sh");
        std::fs::write(&script, "exec bash -lc \"$1\"\n").expect("script");
        let spawn = ConfinedSpawn::new(&[]).expect("unconfined spawn");
        let outcome = run_in_pty(
            PtyRequest {
                program: Path::new("bash"),
                script: &script,
                command: "printf 'x%.0s' $(seq 1 5000)",
                project_root: &directory,
                max_output_bytes: 512,
                max_execution_time: Duration::from_secs(10),
            },
            &spawn,
        )
        .await
        .expect("pty run");
        assert!(outcome.truncated, "expected truncation");
        assert!(outcome.bytes.len() <= 512);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn a_command_that_outlives_its_budget_is_reported_as_timed_out() {
        let directory = scratch("timeout");
        let script = directory.join("main.sh");
        std::fs::write(&script, "exec bash -lc \"$1\"\n").expect("script");
        let spawn = ConfinedSpawn::new(&[]).expect("unconfined spawn");
        let error = run_in_pty(
            PtyRequest {
                program: Path::new("bash"),
                script: &script,
                command: "sleep 30",
                project_root: &directory,
                max_output_bytes: 64 * 1024,
                max_execution_time: Duration::from_millis(300),
            },
            &spawn,
        )
        .await
        .expect_err("expected a timeout");
        assert!(error.message().contains("timed out"), "{error}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}
