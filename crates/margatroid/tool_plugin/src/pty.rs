use std::io::Read;
use std::process::Stdio;
use tokio::io::unix::AsyncFd;

pub(crate) fn configure(
    command: &mut tokio::process::Command,
) -> std::io::Result<AsyncFd<std::fs::File>> {
    let terminal = nix::pty::openpty(
        Some(&nix::pty::Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )?;
    let master = std::fs::File::from(terminal.master);
    let slave = std::fs::File::from(terminal.slave);
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
                return Err(std::io::Error::last_os_error());
            }
            if nix::libc::ioctl(0, nix::libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    AsyncFd::new(master)
}

pub(crate) async fn drain(
    master: AsyncFd<std::fs::File>,
    limit: usize,
) -> mlua::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        let mut ready = master.readable().await.map_err(mlua::Error::external)?;
        match ready.try_io(|fd| {
            let mut file = fd.get_ref();
            file.read(&mut chunk)
        }) {
            Ok(Ok(0)) => break,
            Ok(Ok(count)) => {
                let retained = limit.saturating_sub(bytes.len()).min(count);
                bytes.extend_from_slice(&chunk[..retained]);
                truncated |= retained < count;
            }
            Ok(Err(error)) if error.raw_os_error() == Some(nix::libc::EIO) => break,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Ok(Err(error)) => return Err(mlua::Error::external(error)),
            Err(_) => continue,
        }
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}
