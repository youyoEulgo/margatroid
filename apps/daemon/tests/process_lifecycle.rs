#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn sigterm_stops_daemon_cleanly() {
    assert_signal_shutdown("TERM");
}

#[test]
fn sigint_stops_daemon_cleanly() {
    assert_signal_shutdown("INT");
}

#[test]
fn bind_failure_returns_nonzero_exit() {
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let mut child = spawn_daemon(directory.path(), address);

    let exit = wait_for_exit(&mut child, Duration::from_secs(5));

    assert!(!exit.success());
}

fn assert_signal_shutdown(signal: &str) {
    let directory = tempfile::tempdir().unwrap();
    let address = available_address();
    let mut child = spawn_daemon(directory.path(), address);
    wait_until_ready(&mut child, address);

    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(status.success());

    let exit = wait_for_exit(&mut child, Duration::from_secs(5));
    assert!(exit.success(), "daemon exited with {exit}");
}

fn available_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

fn spawn_daemon(data_dir: &Path, address: SocketAddr) -> Child {
    Command::new(env!("CARGO_BIN_EXE_margatroidd"))
        .args(["--bind", &address.to_string(), "--data-dir"])
        .arg(data_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_until_ready(child: &mut Child, address: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("daemon exited before readiness: {status}");
        }
        if readiness_response(address).is_some_and(|response| response.starts_with("HTTP/1.1 200"))
        {
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("daemon readiness timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn readiness_response(address: SocketAddr) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(50)).ok()?;
    stream
        .write_all(b"GET /ready HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("daemon did not exit after signal");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
