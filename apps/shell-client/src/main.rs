use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use std::process::{ExitStatus, Stdio};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

mod output;
#[cfg(target_os = "linux")]
mod pty;

type Input = BufReader<tokio::io::Stdin>;
type Frames = mpsc::Sender<Value>;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    Exec {
        command: String,
        max_output_bytes: usize,
        #[serde(default)]
        pty: bool,
    },
    Terminal {
        max_output_bytes: usize,
    },
    Write {
        text: String,
    },
    Close,
}

fn failure(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

async fn request(input: &mut Input) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if input.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|e| failure(e.to_string()))
}

async fn frame(tx: &Frames, value: Value) -> io::Result<()> {
    tx.send(value)
        .await
        .map_err(|_| failure("protocol output closed"))
}

async fn cancelled(input: &mut Input) -> io::Result<()> {
    match request(input).await? {
        None | Some(Request::Close) => Ok(()),
        _ => Err(failure("exec accepts only close after its initial request")),
    }
}

#[cfg(unix)]
async fn shutdown() -> io::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::select! { _ = term.recv() => {}, _ = interrupt.recv() => {}, _ = hangup.recv() => {} }
    Ok(())
}
#[cfg(not(unix))]
async fn shutdown() -> io::Result<()> {
    std::future::pending().await
}

fn group(command: &mut Command) {
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if nix::libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = command;
}

async fn cleanup(child: &mut Child, pid: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        // The group remains addressable after the direct child has exited.
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(-(pid as i32)),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.start_kill();
    child.wait().await?;
    Ok(())
}

async fn capture<R: AsyncRead + Unpin>(mut pipe: R, maximum: usize) -> io::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        let count = pipe.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let retained = maximum.saturating_sub(bytes.len()).min(count);
        bytes.extend_from_slice(&chunk[..retained]);
        truncated |= retained < count;
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

fn result(
    status: ExitStatus,
    stdout: String,
    stderr: String,
    truncated: bool,
    terminal: bool,
) -> Value {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    json!({"type":"result", "stdout":stdout, "stderr":stderr,
        "code":status.code().unwrap_or(-1), "exit_code":status.code().unwrap_or(-1),
        "signal":signal, "truncated":truncated, "terminal":terminal})
}

async fn exec(
    input: &mut Input,
    tx: &Frames,
    script: String,
    maximum: usize,
    terminal: bool,
) -> io::Result<()> {
    if script.trim().is_empty() {
        return Err(failure("shell command is empty"));
    }
    #[cfg(not(unix))]
    return Err(failure("shell execution requires Unix"));
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(target_os = "linux")]
        let master = if terminal {
            Some(pty::configure(&mut command)?)
        } else {
            group(&mut command);
            None
        };
        #[cfg(not(target_os = "linux"))]
        if terminal {
            return Err(failure("PTY requires Linux"));
        } else {
            group(&mut command);
        }
        let mut child = command.spawn()?;
        drop(command);
        let pid = child.id().expect("spawned child id");
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let work = async {
            let output = async {
                #[cfg(target_os = "linux")]
                if let Some(master) = master {
                    let (text, truncated) = pty::drain(master, maximum).await?;
                    return Ok::<_, io::Error>((text, String::new(), truncated));
                }
                let ((out, a), (err, b)) = tokio::try_join!(
                    capture(stdout.expect("stdout piped"), maximum),
                    capture(stderr.expect("stderr piped"), maximum)
                )?;
                Ok((out, err, a || b))
            };
            let (status, (out, err, truncated)) = tokio::try_join!(child.wait(), output)?;
            Ok::<_, io::Error>(result(status, out, err, truncated, terminal))
        };
        let outcome = tokio::select! {
            value = work => value.map(Some),
            cancel = cancelled(input) => cancel.map(|_| None),
            signal = shutdown() => signal.map(|_| None),
            _ = tx.closed() => Err(failure("protocol output closed")),
        };
        match outcome {
            Ok(Some(value)) => frame(tx, value).await,
            other => {
                cleanup(&mut child, pid).await?;
                other.map(|_| ())
            }
        }
    }
}

async fn run(input: &mut Input, tx: &Frames) -> io::Result<()> {
    match request(input).await? {
        Some(Request::Exec {
            command,
            max_output_bytes,
            pty,
        }) => exec(input, tx, command, max_output_bytes, pty).await,
        Some(Request::Terminal { max_output_bytes }) => {
            #[cfg(target_os = "linux")]
            {
                pty::terminal(input, tx, max_output_bytes).await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = max_output_bytes;
                Err(failure("PTY requires Linux"))
            }
        }
        _ => Err(failure("first request must be exec or terminal")),
    }
}

#[tokio::main]
async fn main() {
    let (tx, mut rx) = mpsc::channel::<Value>(16);
    // This task is the only owner of stdout; backpressure never holds a child lock.
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(value) = rx.recv().await {
            let mut bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
            bytes.push(b'\n');
            stdout.write_all(&bytes).await?;
            stdout.flush().await?;
        }
        Ok::<_, io::Error>(())
    });
    let mut input = BufReader::new(tokio::io::stdin());
    let outcome = run(&mut input, &tx).await;
    if let Err(error) = &outcome {
        eprintln!("shell-client: {error}");
        let _ = frame(&tx, json!({"type":"error", "message":error.to_string()})).await;
    }
    drop(tx);
    let written = writer.await;
    let code = if outcome.is_err() || !matches!(written, Ok(Ok(()))) {
        1
    } else {
        0
    };
    // Tokio stdin may retain a blocking read after its future is cancelled.
    // All owned children and the protocol writer have already been awaited.
    std::process::exit(code);
}
