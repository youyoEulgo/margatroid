#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_runtime_plugin::{AppControl, AppRunExt, AppRuntimePlugin};
use core_plugin::{App, Stage, World};
use external_event_plugin::ExternalEventPlugin;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use terminal_input_plugin::{
    KeyCode, KeyModifiers, TerminalEvent, TerminalInputFailed, TerminalInputFailureKind,
    TerminalInputOptions, TerminalInputPlugin,
};

const FIXTURE_ENV: &str = "MECS_TERMINAL_PTY_FIXTURE";
const READY_ENV: &str = "MECS_TERMINAL_PTY_READY";
const RESULT_ENV: &str = "MECS_TERMINAL_PTY_RESULT";
const MODE_ENV: &str = "MECS_TERMINAL_PTY_MODE";
const NON_TTY_FIXTURE_ENV: &str = "MECS_TERMINAL_NON_TTY_FIXTURE";

#[test]
fn raw_terminal_reads_keys_and_resize_then_restores_mode() {
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("ready");
    let result = directory.path().join("result");
    let system = native_pty_system();
    let pair = system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args(["--exact", "pty_fixture", "--nocapture", "--test-threads=1"]);
    command.env(FIXTURE_ENV, "1");
    command.env(READY_ENV, &ready);
    command.env(RESULT_ENV, &result);
    command.env(MODE_ENV, "raw");
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let drain = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        output
    });
    let mut writer = pair.master.take_writer().unwrap();

    wait_for_path(&mut child, &ready);
    writer.write_all(b"a\x03").unwrap();
    writer.flush().unwrap();
    pair.master
        .resize(PtySize {
            rows: 37,
            cols: 111,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    writer.write_all(b"q").unwrap();
    writer.flush().unwrap();
    drop(writer);

    let status = wait_for_exit(&mut child, &result);
    let _output = drain.join().unwrap();
    assert!(status.success(), "fixture failed: {status:?}");
    let observed = fs::read_to_string(result).unwrap();
    assert!(observed.contains("key:a"), "{observed}");
    assert!(observed.contains("key:ctrl-c"), "{observed}");
    assert!(observed.contains("resize:111x37"), "{observed}");
    assert!(observed.contains("restored"), "{observed}");
}

#[test]
fn cooked_terminal_reads_line_and_end_of_input() {
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("ready");
    let result = directory.path().join("result");
    let system = native_pty_system();
    let pair = system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args(["--exact", "pty_fixture", "--nocapture", "--test-threads=1"]);
    command.env(FIXTURE_ENV, "1");
    command.env(READY_ENV, &ready);
    command.env(RESULT_ENV, &result);
    command.env(MODE_ENV, "cooked");
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let drain = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        output
    });
    let mut writer = pair.master.take_writer().unwrap();

    wait_for_path(&mut child, &ready);
    writer.write_all(b"hello\n").unwrap();
    writer.flush().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    writer.write_all(b"\x04").unwrap();
    writer.flush().unwrap();
    drop(writer);

    let status = wait_for_exit(&mut child, &result);
    let _output = drain.join().unwrap();
    assert!(status.success(), "fixture failed: {status:?}");
    let observed = fs::read_to_string(result).unwrap();
    assert!(observed.contains("line:hello"), "{observed}");
    assert!(observed.contains("eof"), "{observed}");
    assert!(observed.contains("restored"), "{observed}");
}

#[test]
fn non_tty_stdin_produces_observable_failure() {
    let directory = tempfile::tempdir().unwrap();
    let result = directory.path().join("result");
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "non_tty_fixture",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(NON_TTY_FIXTURE_ENV, "1")
        .env(RESULT_ENV, &result)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(fs::read_to_string(result).unwrap(), "not-a-terminal");
}

#[test]
fn non_tty_fixture() {
    if std::env::var_os(NON_TTY_FIXTURE_ENV).is_none() {
        return;
    }
    let result = PathBuf::from(std::env::var_os(RESULT_ENV).unwrap());
    let mut app = App::new();
    app.add_plugins(ExternalEventPlugin);
    app.add_plugins(TerminalInputPlugin::with_options(
        TerminalInputOptions::raw(),
    ));
    let mut reader = app.event_reader::<TerminalInputFailed>();
    app.tick();
    let failures = app.world().read_events(&mut reader);
    let observed = match failures.first().map(|failure| failure.kind) {
        Some(TerminalInputFailureKind::NotATerminal) => "not-a-terminal",
        _ => "unexpected",
    };
    fs::write(result, observed).unwrap();
}

#[test]
fn pty_fixture() {
    if std::env::var_os(FIXTURE_ENV).is_none() {
        return;
    }
    let ready = PathBuf::from(std::env::var_os(READY_ENV).unwrap());
    let result = PathBuf::from(std::env::var_os(RESULT_ENV).unwrap());
    let mode = std::env::var(MODE_ENV).unwrap();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));

    let mut app = App::new();
    app.add_plugins(AppRuntimePlugin);
    app.add_plugins(ExternalEventPlugin);
    let options = match mode.as_str() {
        "raw" => TerminalInputOptions::raw(),
        "cooked" => TerminalInputOptions::cooked(),
        _ => panic!("unknown fixture mode"),
    };
    app.add_plugins(TerminalInputPlugin::with_options(options));
    app.add_systems(
        Stage::Startup,
        [move |_world: &mut World| {
            fs::write(&ready, b"ready").unwrap();
        }],
    );

    let control = app.world().resource::<AppControl>().unwrap().clone();
    let mut reader = app.event_reader::<TerminalEvent>();
    let system_seen = seen.clone();
    let live_result = result.clone();
    app.add_systems(
        Stage::Update,
        [move |world: &mut World| {
            for event in world.read_events(&mut reader) {
                match event {
                    TerminalEvent::Key(key) if key.code == KeyCode::Char('a') => {
                        system_seen.lock().unwrap().push("key:a".into());
                    }
                    TerminalEvent::Key(key)
                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        system_seen.lock().unwrap().push("key:ctrl-c".into());
                    }
                    TerminalEvent::Key(key) if key.code == KeyCode::Char('q') => {
                        control.shutdown();
                    }
                    TerminalEvent::Line(line) => {
                        system_seen.lock().unwrap().push(format!("line:{line}"));
                    }
                    TerminalEvent::EndOfInput => {
                        system_seen.lock().unwrap().push("eof".into());
                        control.shutdown();
                    }
                    TerminalEvent::Resize(size) => {
                        system_seen
                            .lock()
                            .unwrap()
                            .push(format!("resize:{}x{}", size.columns, size.rows));
                    }
                    _ => {}
                }
                fs::write(&live_result, system_seen.lock().unwrap().join("\n")).unwrap();
            }
        }],
    );

    app.run();
    let restored = !crossterm::terminal::is_raw_mode_enabled().unwrap_or(true);
    let mut observed = seen.lock().unwrap().join("\n");
    if restored {
        observed.push_str("\nrestored");
    }
    fs::write(result, observed).unwrap();
}

fn wait_for_path(child: &mut Box<dyn portable_pty::Child + Send + Sync>, path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("fixture exited before startup: {status:?}");
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("fixture startup timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_exit(
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
    result: &Path,
) -> portable_pty::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let observed = fs::read_to_string(result).unwrap_or_else(|_| "<no result>".into());
            panic!("fixture shutdown timed out; observed: {observed}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
