use std::error::Error;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use compose::compile;
use futures_util::{SinkExt, StreamExt};
use margatroid_protocol::{ClientMessage, ServerMessage};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const DEFAULT_WORKSPACE_FILE: &str = "margatroid-workspace.yaml";
const DEFAULT_BACKEND_URL: &str = "ws://127.0.0.1:3939/ws";
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    WorkspaceUp {
        workspace_file: PathBuf,
        backend_url: String,
    },
}

#[tokio::main]
async fn main() {
    match parse_args(std::env::args().skip(1)) {
        Ok(Command::Help) => println!("{}", usage()),
        Ok(command) => {
            if let Err(error) = run(command).await {
                print_cli_event("ERROR", &format!("command failed ({error})"));
                process::exit(1);
            }
        }
        Err(error) => {
            print_cli_event("ERROR", &format!("argument parsing failed ({error})"));
            eprintln!();
            eprintln!("{}", usage());
            process::exit(2);
        }
    }
}

async fn run(command: Command) -> Result<(), Box<dyn Error + Send + Sync>> {
    match command {
        Command::Help => Ok(()),
        Command::WorkspaceUp {
            workspace_file,
            backend_url,
        } => run_workspace_up(workspace_file, backend_url).await,
    }
}

async fn run_workspace_up(
    workspace_file: PathBuf,
    backend_url: String,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let definition = compile(&workspace_file)?;
    let start_request_id = request_id();
    let registration = serde_json::to_string(&ClientMessage::register_connection(
        format!("{start_request_id}-register"),
        "cli",
    ))?;
    let request = ClientMessage::start_workspace(&start_request_id, &definition);
    let encoded = serde_json::to_string(&request)?;
    let workspace = margatroid_protocol::WorkspaceReferenceDto::new(
        definition.name.clone(),
        definition.project_root.to_string_lossy(),
    );

    print_cli_event(
        "INFO",
        &format!(
            "workspace compiled (file={}, workspace={})",
            workspace_file.display(),
            definition.name
        ),
    );
    print_cli_event(
        "INFO",
        &format!("connecting to backend (url={backend_url})"),
    );
    let (mut socket, _) = connect_async(&backend_url).await?;
    print_cli_event("INFO", "backend WebSocket connected");
    socket.send(Message::Text(registration.into())).await?;
    print_cli_event("INFO", "connection.register sent (client_type=cli)");
    socket.send(Message::Text(encoded.into())).await?;
    print_cli_event(
        "INFO",
        &format!("workspace.start sent (request_id={start_request_id})"),
    );

    loop {
        tokio::select! {
            signal = wait_for_shutdown_signal() => {
                signal?;
                let stop_id = request_id();
                let stop = ClientMessage::stop_workspace(&stop_id, &workspace);
                socket.send(Message::Text(serde_json::to_string(&stop)?.into())).await?;
                print_cli_event("INFO", &format!("workspace.stop sent (request_id={stop_id})"));
                wait_for_stop_ack(&mut socket, &stop_id).await?;
                print_cli_event("INFO", &format!("workspace stopped (request_id={stop_id})"));
                socket.close(None).await?;
                print_cli_event("INFO", "backend WebSocket closed");
                return Ok(());
            }
            message = socket.next() => {
                let Some(message) = message else { return Ok(()); };
                match message? {
                    Message::Text(text) => print_backend_message(&text),
                    Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(text) => print_backend_message(&text),
                        Err(_) => print_cli_event("WARN", &format!("non-UTF-8 backend message ignored (bytes={})", bytes.len())),
                    },
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                    Message::Pong(_) => {},
                    Message::Close(frame) => {
                        if let Some(frame) = frame {
                            print_cli_event("INFO", &format!("backend closed WebSocket (code={}, reason={})", frame.code, frame.reason));
                        } else {
                            print_cli_event("INFO", "backend closed WebSocket");
                        }
                        return Ok(());
                    }
                    Message::Frame(_) => {},
                }
            }
        }
    }
}

async fn wait_for_stop_ack(
    socket: &mut (impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
              + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
              + Unpin),
    request_id: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut deadline = Box::pin(tokio::time::sleep(Duration::from_secs(5)));
    loop {
        tokio::select! {
            _ = &mut deadline => return Err("timed out waiting for workspace.stopped".into()),
            signal = wait_for_shutdown_signal() => {
                signal?;
                futures_util::SinkExt::close(socket).await?;
                return Err("shutdown requested again before workspace stopped".into());
            }
            message = socket.next() => {
                let Some(message) = message else { return Err("backend disconnected before workspace stopped".into()); };
                match message? {
                    Message::Text(text) => {
                        print_backend_message(&text);
                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(ServerMessage::WorkspaceStopped { id, .. }) if id == request_id => return Ok(()),
                            Ok(ServerMessage::WorkspaceStopFailed { id, error }) if id == request_id => {
                                return Err(format!("workspace stop failed: {error}").into());
                            }
                            _ => {}
                        }
                    }
                    Message::Binary(bytes) => if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                        print_backend_message(&text);
                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(ServerMessage::WorkspaceStopped { id, .. }) if id == request_id => return Ok(()),
                            Ok(ServerMessage::WorkspaceStopFailed { id, error }) if id == request_id => {
                                return Err(format!("workspace stop failed: {error}").into());
                            }
                            _ => {}
                        }
                    },
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                    Message::Pong(_) => {},
                    Message::Close(_) => return Err("backend closed before workspace stopped".into()),
                    Message::Frame(_) => {},
                }
            }
        }
    }
}

async fn wait_for_shutdown_signal() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

fn print_backend_message(text: &str) {
    let Ok(ServerMessage::Log { record }) = serde_json::from_str(text) else {
        return;
    };
    let fields = record
        .fields
        .iter()
        .filter(|field| field.name != "message")
        .map(|field| format!("{}={}", field.name, field.value))
        .collect::<Vec<_>>();
    let fields = if fields.is_empty() {
        String::new()
    } else {
        format!(" ({})", fields.join(", "))
    };
    println!(
        "{}",
        format_log_line(
            record.timestamp_millis,
            &record.level,
            &record.target,
            &record.message,
            &fields,
            io::stdout().is_terminal(),
        )
    );
}

fn print_cli_event(level: &str, message: &str) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    eprintln!(
        "{}",
        format_log_line(
            timestamp,
            level,
            "margatroid_cli",
            message,
            "",
            io::stderr().is_terminal(),
        )
    );
}

fn format_log_line(
    timestamp_millis: u64,
    level: &str,
    target: &str,
    message: &str,
    fields: &str,
    ansi: bool,
) -> String {
    let timestamp = format_timestamp(timestamp_millis);
    if !ansi {
        return format!("{timestamp} {level:<5} {target}: {message}{fields}");
    }
    let level_color = match level {
        "ERROR" => "\x1b[31m",
        "WARN" => "\x1b[33m",
        "INFO" => "\x1b[32m",
        "DEBUG" => "\x1b[34m",
        "TRACE" => "\x1b[35m",
        _ => ANSI_RESET,
    };
    format!(
        "{ANSI_DIM}{timestamp}{ANSI_RESET} {level_color}{level:<5}{ANSI_RESET} {ANSI_DIM}{target}:{ANSI_RESET} {message}{fields}"
    )
}

fn format_timestamp(timestamp_millis: u64) -> String {
    let nanos = i128::from(timestamp_millis).saturating_mul(1_000_000);
    OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .ok()
        .and_then(|timestamp| timestamp.format(&Rfc3339).ok())
        .unwrap_or_else(|| timestamp_millis.to_string())
}

fn parse_args<I>(arguments: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Err("a command is required".into());
    };
    if command == "--help" || command == "-h" {
        return Ok(Command::Help);
    }
    if command != "workspace" {
        return Err(format!("unknown command '{command}'"));
    }

    let Some(action) = arguments.next() else {
        return Err("workspace action is required".into());
    };
    if action == "--help" || action == "-h" {
        return Ok(Command::Help);
    }
    if action != "up" {
        return Err(format!("unknown workspace action '{action}'"));
    }

    let mut workspace_file = None;
    let mut backend_url = DEFAULT_BACKEND_URL.to_owned();
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Ok(Command::Help);
        }
        if argument == "--backend" {
            backend_url = arguments
                .next()
                .ok_or_else(|| "--backend requires a WebSocket URL".to_owned())?;
            continue;
        }
        if let Some(value) = argument.strip_prefix("--backend=") {
            if value.is_empty() {
                return Err("--backend requires a WebSocket URL".into());
            }
            backend_url = value.to_owned();
            continue;
        }
        if argument.starts_with('-') {
            return Err(format!("unknown option '{argument}'"));
        }
        if workspace_file.is_some() {
            return Err("workspace file was provided more than once".into());
        }
        workspace_file = Some(PathBuf::from(argument));
    }

    Ok(Command::WorkspaceUp {
        workspace_file: workspace_file.unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_FILE)),
        backend_url,
    })
}

fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("cli-{}-{timestamp}", process::id())
}

fn usage() -> &'static str {
    "Usage: margatroid workspace up [WORKSPACE_FILE] [--backend WS_URL]\n\nCompile a workspace file, send it to the backend, and print backend WebSocket messages."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_workspace_file_and_local_backend() {
        assert_eq!(
            parse_args(["workspace".into(), "up".into()]).unwrap(),
            Command::WorkspaceUp {
                workspace_file: PathBuf::from(DEFAULT_WORKSPACE_FILE),
                backend_url: DEFAULT_BACKEND_URL.into(),
            }
        );
    }

    #[test]
    fn accepts_file_and_backend_options() {
        assert_eq!(
            parse_args([
                "workspace".into(),
                "up".into(),
                "project/workspace.yaml".into(),
                "--backend".into(),
                "ws://localhost:4000/events".into(),
            ])
            .unwrap(),
            Command::WorkspaceUp {
                workspace_file: PathBuf::from("project/workspace.yaml"),
                backend_url: "ws://localhost:4000/events".into(),
            }
        );
    }

    #[test]
    fn backend_log_timestamp_is_rfc3339() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_timestamp(1_786_281_543_032),
            "2026-08-09T13:19:03.032Z"
        );
    }

    #[test]
    fn terminal_log_line_colors_timestamp_level_and_target() {
        let line = format_log_line(0, "INFO", "dto_plugin::outbound", "started", "", true);
        assert_eq!(
            line,
            "\x1b[2m1970-01-01T00:00:00Z\x1b[0m \x1b[32mINFO \x1b[0m \x1b[2mdto_plugin::outbound:\x1b[0m started"
        );
    }

    #[test]
    fn redirected_log_line_has_no_ansi_codes() {
        let line = format_log_line(0, "INFO", "margatroid_cli", "started", "", false);
        assert_eq!(line, "1970-01-01T00:00:00Z INFO  margatroid_cli: started");
    }
}
