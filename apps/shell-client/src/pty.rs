use crate::{failure, frame, request, Frames, Input, Request};
use serde_json::json;
use std::{fs::File, io, process::Stdio, sync::Arc};
use tokio::io::unix::AsyncFd;
use tokio::process::Command;

type Master = Arc<AsyncFd<File>>;

#[allow(deprecated)]
pub(crate) fn configure(command: &mut Command) -> io::Result<Master> {
    let terminal = nix::pty::openpty(
        Some(&nix::pty::Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )?;
    let master = File::from(terminal.master);
    let slave = File::from(terminal.slave);
    let flags = nix::fcntl::fcntl(&master, nix::fcntl::FcntlArg::F_GETFL)?;
    nix::fcntl::fcntl(
        &master,
        nix::fcntl::FcntlArg::F_SETFL(
            nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK,
        ),
    )?;
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    unsafe {
        command.pre_exec(|| {
            if nix::libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if nix::libc::ioctl(0, nix::libc::TIOCSCTTY, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(Arc::new(AsyncFd::new(master)?))
}

async fn read(master: &Master, chunk: &mut [u8]) -> io::Result<usize> {
    loop {
        let mut ready = master.readable().await?;
        match ready.try_io(|fd| std::io::Read::read(&mut fd.get_ref(), chunk)) {
            Ok(Ok(count)) => return Ok(count),
            Ok(Err(error)) if error.raw_os_error() == Some(nix::libc::EIO) => return Ok(0),
            Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {}
        }
    }
}

async fn write(master: &Master, bytes: &[u8]) -> io::Result<()> {
    let mut position = 0;
    while position < bytes.len() {
        let mut ready = master.writable().await?;
        match ready.try_io(|fd| std::io::Write::write(&mut fd.get_ref(), &bytes[position..])) {
            Ok(Ok(0)) => return Err(failure("terminal input closed")),
            Ok(Ok(count)) => position += count,
            Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {}
        }
    }
    Ok(())
}

pub(crate) async fn drain(master: Master, maximum: usize) -> io::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        let count = read(&master, &mut chunk).await?;
        if count == 0 {
            break;
        }
        let retained = maximum.saturating_sub(bytes.len()).min(count);
        bytes.extend_from_slice(&chunk[..retained]);
        truncated |= retained < count;
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

async fn stream(
    master: &Master,
    tx: &Frames,
    output: &mut crate::output::TerminalOutput,
    pending: &mut Option<serde_json::Value>,
) -> io::Result<()> {
    let mut chunk = [0u8; 8192];
    loop {
        let count = read(master, &mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        let (text, replace) = output.process(&chunk[..count])?;
        *pending = Some(json!({"type":"output", "text":text, "replace":replace}));
        frame(tx, pending.as_ref().expect("pending output frame").clone()).await?;
        *pending = None;
    }
}

fn available(master: &Master) -> io::Result<usize> {
    use std::os::fd::AsRawFd;
    let mut count: nix::libc::c_int = 0;
    if unsafe {
        nix::libc::ioctl(
            master.get_ref().as_raw_fd(),
            nix::libc::FIONREAD,
            &mut count,
        )
    } == -1
    {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(nix::libc::EIO) {
            return Ok(0);
        }
        return Err(error);
    }
    Ok(count.max(0) as usize)
}

pub(crate) async fn terminal(input: &mut Input, tx: &Frames, maximum: usize) -> io::Result<()> {
    let mut command = Command::new("bash");
    command
        .args(["--noprofile", "--norc", "-i"])
        .env("TERM", "xterm-256color")
        .kill_on_drop(true);
    let master = configure(&mut command)?;
    let mut child = command.spawn()?;
    drop(command);
    let pid = child.id().expect("spawned child id");
    let work = async {
        frame(tx, json!({"type":"ready"})).await?;
        // Keep a frame until it has reached the queue: cancelling a blocked
        // send when child.wait completes must not lose its rendered bytes.
        let mut output = crate::output::TerminalOutput::new(maximum);
        let mut pending = None;
        let writer = async {
            loop {
                match request(input).await? {
                    None | Some(Request::Close) => return Ok::<_, io::Error>(()),
                    Some(Request::Write { text }) => write(&master, text.as_bytes()).await?,
                    _ => return Err(failure("terminal accepts only write or close")),
                }
            }
        };
        let completion = async {
            let status = tokio::select! {
                status = child.wait() => status?,
                drained = stream(&master, tx, &mut output, &mut pending) => {
                    drained?;
                    child.wait().await?
                }
            };
            // A disowned descendant can retain the slave indefinitely. Drain
            // only the bytes already buffered when the direct child exited.
            let mut remaining = available(&master)?;
            if let Some(value) = pending.take() {
                frame(tx, value).await?;
            }
            let mut chunk = [0u8; 8192];
            while remaining > 0 {
                let size = remaining.min(chunk.len());
                let count = match std::io::Read::read(&mut master.get_ref(), &mut chunk[..size]) {
                    Ok(count) => count,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            || error.raw_os_error() == Some(nix::libc::EIO) =>
                    {
                        break
                    }
                    Err(error) => return Err(error),
                };
                if count == 0 {
                    break;
                }
                remaining -= count;
                let (text, replace) = output.process(&chunk[..count])?;
                frame(tx, json!({"type":"output", "text":text, "replace":replace})).await?;
            }
            Ok::<_, io::Error>(Some(status))
        };
        tokio::select! {
            status = completion => status,
            done = writer => done.map(|_| None),
            signal = crate::shutdown() => signal.map(|_| None),
            _ = tx.closed() => Err(failure("protocol output closed")),
        }
    }
    .await;
    let status = match work {
        Ok(Some(status)) => status,
        other => {
            crate::cleanup(&mut child, pid).await?;
            other?;
            child.wait().await?
        }
    };
    frame(
        tx,
        json!({"type":"exit", "eof":true, "exit_code":status.code()}),
    )
    .await
}
