//! The one place a command runs behind a pseudoterminal.
//!
//! A tool that needs a terminal — an interactive program, or one that branches
//! on `isatty` — cannot be served by a pipe. Both the shell tool and the Lua
//! tool route through here, so a command behaves the same whichever tool asked
//! for it, and the process-group, timeout and output limits have one
//! implementation.
//!
//! A sandbox may withhold `/dev/ptmx`, which is what allocating a terminal
//! needs. Rather than fail the call, the run falls back to pipes: the command
//! still executes, it simply sees no terminal.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

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

/// What a finished run produced. `exit_code` is absent when the child was
/// killed by a signal, which is how a timeout surfaces here.
#[derive(Debug)]
pub(crate) struct PtyOutcome {
    pub(crate) exit_code: Option<i32>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
    /// Whether the command ran with a terminal, so a caller that needs one can
    /// tell the difference instead of assuming.
    pub(crate) terminal: bool,
}

/// How the command's streams are wired.
enum Streams {
    /// One terminal: the child sees a tty and both streams arrive on the master.
    Terminal(tokio::fs::File),
    /// Separate pipes: no tty, and the streams are read apart then merged.
    Pipes {
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
    },
}

pub(crate) async fn run_in_pty(
    request: PtyRequest<'_>,
    spawn: &ConfinedSpawn,
) -> Result<PtyOutcome, ToolError> {
    let (mut child, streams) = spawn_command(request, spawn)?;
    let mut group = ProcessGroup::confined(spawn.is_confined(), child.id().unwrap_or_default());
    let terminal = matches!(streams, Streams::Terminal(_));

    let reading = collect(streams, request.max_output_bytes);

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
    let (bytes, truncated) = reading.await;
    Ok(PtyOutcome {
        exit_code: status.code(),
        bytes,
        truncated,
        terminal,
    })
}

async fn collect(streams: Streams, limit: usize) -> (Vec<u8>, bool) {
    let mut output = Vec::new();
    let mut truncated = false;
    match streams {
        Streams::Terminal(mut master) => {
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
        }
        Streams::Pipes { stdout, stderr } => {
            // A terminal merges the two streams, so the pipe path merges them
            // too: a caller reads one stream either way. stderr is drained into
            // its own buffer so neither pipe can block the other.
            let mut errors = Vec::new();
            let mut errors_truncated = false;
            tokio::join!(
                drain(stdout, limit, &mut output, &mut truncated),
                drain(stderr, limit, &mut errors, &mut errors_truncated),
            );
            extend_bounded(&mut output, &errors, limit, &mut truncated);
        }
    }
    (output, truncated)
}

async fn drain(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
    output: &mut Vec<u8>,
    truncated: &mut bool,
) {
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => extend_bounded(output, &buffer[..read], limit, truncated),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn spawn_command(
    request: PtyRequest<'_>,
    spawn: &ConfinedSpawn,
) -> Result<(tokio::process::Child, Streams), ToolError> {
    match open_terminal() {
        Some((master, stdin, stdout, stderr, slave_fd)) => {
            let mut process = spawn.command(request.program, request.project_root)?;
            process
                .arg(request.script)
                .arg(request.command)
                .stdin(stdin)
                .stdout(stdout)
                .stderr(stderr)
                .kill_on_drop(true);
            // A terminal needs a real type; the pipe path leaves TERM=dumb in
            // place so captured output stays plain.
            process.env("TERM", "xterm-256color");
            // The child takes its own session and makes the slave its controlling
            // terminal. Without this the command shares the runner's session, and
            // a terminal it never properly acquired misreports its own state.
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
            let child = start(process)?;
            Ok((child, Streams::Terminal(master)))
        }
        None => {
            let mut process = spawn.command(request.program, request.project_root)?;
            process
                .arg(request.script)
                .arg(request.command)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = start(process)?;
            let stdout = child.stdout.take().ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    "Shell process stdout is unavailable",
                )
            })?;
            let stderr = child.stderr.take().ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    "Shell process stderr is unavailable",
                )
            })?;
            Ok((child, Streams::Pipes { stdout, stderr }))
        }
    }
}

fn start(mut process: tokio::process::Command) -> Result<tokio::process::Child, ToolError> {
    process.spawn().map_err(|error| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            format!("Shell process could not be started: {error}"),
        )
    })
}

/// A terminal, or nothing when the sandbox withholds the multiplexer. A sandbox
/// that denies `/dev/ptmx` makes `openpty` fail, and that is not an error worth
/// failing a tool call over — the command runs on pipes instead.
fn open_terminal() -> Option<(tokio::fs::File, Stdio, Stdio, Stdio, std::os::fd::RawFd)> {
    let pty = nix::pty::openpty(None, None).ok()?;
    let master = tokio::fs::File::from_std(std::fs::File::from(pty.master));
    let slave_fd = std::os::fd::AsRawFd::as_raw_fd(&pty.slave);
    let stdin = Stdio::from(pty.slave.try_clone().ok()?);
    let stdout = Stdio::from(pty.slave.try_clone().ok()?);
    let stderr = Stdio::from(pty.slave);
    Some((master, stdin, stdout, stderr, slave_fd))
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

    async fn run(
        directory: &Path,
        command: &str,
        bytes: usize,
        ms: u64,
    ) -> Result<PtyOutcome, ToolError> {
        let script = directory.join(SHELL_SCRIPT_FILE);
        std::fs::write(&script, "exec bash -c \"$1\"\n").expect("script");
        let spawn = ConfinedSpawn::new(&[]).expect("unconfined spawn");
        run_in_pty(
            PtyRequest {
                program: Path::new("bash"),
                script: &script,
                command,
                project_root: directory,
                max_output_bytes: bytes,
                max_execution_time: Duration::from_millis(ms),
            },
            &spawn,
        )
        .await
    }

    #[tokio::test]
    async fn the_child_sees_a_terminal_when_one_is_available() {
        let directory = scratch("tty");
        let outcome = run(
            &directory,
            "test -t 1 && echo tty=yes || echo tty=no",
            64 * 1024,
            10_000,
        )
        .await
        .expect("run");
        let text = normalize_pty_text(&outcome.bytes);
        if outcome.terminal {
            assert!(text.contains("tty=yes"), "expected a terminal: {text}");
        }
        assert_eq!(outcome.exit_code, Some(0));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn the_exit_code_and_output_come_back() {
        let directory = scratch("exit");
        let outcome = run(&directory, "echo captured; exit 3", 64 * 1024, 10_000)
            .await
            .expect("run");
        let text = normalize_pty_text(&outcome.bytes);
        assert!(text.contains("captured"), "output missing: {text}");
        assert_eq!(outcome.exit_code, Some(3));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn both_streams_reach_the_caller() {
        // A terminal merges the streams; the pipe fallback has to merge them too
        // or a command's diagnostics would vanish when no terminal is available.
        let directory = scratch("streams");
        let outcome = run(
            &directory,
            "echo to-stdout; echo to-stderr >&2",
            64 * 1024,
            10_000,
        )
        .await
        .expect("run");
        let text = normalize_pty_text(&outcome.bytes);
        assert!(text.contains("to-stdout"), "stdout missing: {text}");
        assert!(text.contains("to-stderr"), "stderr missing: {text}");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn output_beyond_the_limit_is_truncated() {
        let directory = scratch("limit");
        let outcome = run(&directory, "printf 'x%.0s' $(seq 1 5000)", 512, 10_000)
            .await
            .expect("run");
        assert!(outcome.truncated, "expected truncation");
        assert!(outcome.bytes.len() <= 512);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn a_command_that_outlives_its_budget_is_reported_as_timed_out() {
        let directory = scratch("timeout");
        let error = run(&directory, "sleep 30", 64 * 1024, 300)
            .await
            .expect_err("expected a timeout");
        assert!(error.message().contains("timed out"), "{error}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}
