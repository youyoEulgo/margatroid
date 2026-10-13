mod error;

use error::CliError;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use compose::compile;
use futures_util::{SinkExt, StreamExt};
use margatroid_protocol::{ClientInfoDto, ClientMessage, ServerMessage};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const DEFAULT_WORKSPACE_FILE: &str = "margatroid-workspace.yaml";
const DEFAULT_BACKEND_URL: &str = "ws://127.0.0.1:3939/ws";
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    name = "margatroid",
    version,
    about = "Compile Margatroid workspaces and connect them to the backend"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
enum Command {
    /// Compile, start, and manage a Workspace.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
enum WorkspaceCommand {
    /// Compile a Workspace file and start it on the backend.
    Up(WorkspaceUpArgs),
}

#[derive(Debug, Args, PartialEq, Eq)]
struct WorkspaceUpArgs {
    /// Workspace definition to compile.
    #[arg(value_name = "WORKSPACE_FILE", default_value = DEFAULT_WORKSPACE_FILE)]
    workspace_file: PathBuf,
    /// Backend WebSocket endpoint.
    #[arg(long = "backend", value_name = "WS_URL", default_value = DEFAULT_BACKEND_URL)]
    backend_url: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli.command).await {
        print_cli_event("ERROR", &format!("command failed ({error})"));
        process::exit(1);
    }
}

async fn run(command: Command) -> Result<(), CliError> {
    match command {
        Command::Workspace {
            command: WorkspaceCommand::Up(arguments),
        } => run_workspace_up(arguments.workspace_file, arguments.backend_url).await,
    }
}

async fn run_workspace_up(workspace_file: PathBuf, backend_url: String) -> Result<(), CliError> {
    let definition = compile(&workspace_file)?;
    let start_request_id = request_id();
    let register_request_id = format!("{start_request_id}-register");
    let registration = serde_json::to_string(&ClientMessage::register_connection(
        register_request_id.as_str(),
        "cli",
    ))?;
    let request = ClientMessage::start_workspace(&start_request_id, &definition);
    let encoded = serde_json::to_string(&request)?;
    let workspace = margatroid_protocol::WorkspaceReferenceDto::with_id(
        margatroid_protocol::ResourceIdDto(definition.id.to_string()),
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
    let registered = wait_for_register_ack(&mut socket, &register_request_id).await?;
    print_cli_event(
        "INFO",
        &format!(
            "connection registered (client={}, type={}, name={})",
            registered.resource_id, registered.client_type, registered.name
        ),
    );
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
                    Message::Text(text) => {
                        print_backend_message(&text);
                        if let Some(error) = workspace_start_error(&text, &start_request_id) {
                            return Err(CliError::Remote(error));
                        }
                    },
                    Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(text) => {
                            print_backend_message(&text);
                            if let Some(error) = workspace_start_error(&text, &start_request_id) {
                                return Err(CliError::Remote(error));
                            }
                        },
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

fn workspace_start_error(text: &str, request_id: &str) -> Option<String> {
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::WorkspaceStartFailed { id, error }) if id == request_id => Some(format!(
            "workspace start failed: {kind}: {message}",
            kind = error.kind,
            message = error.message
        )),
        _ => None,
    }
}

async fn wait_for_register_ack(
    socket: &mut (impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
              + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
              + Unpin),
    request_id: &str,
) -> Result<ClientInfoDto, CliError> {
    let mut deadline = Box::pin(tokio::time::sleep(Duration::from_secs(5)));
    loop {
        tokio::select! {
            _ = &mut deadline => return Err(CliError::Timeout("timed out waiting for connection.registered")),
            signal = wait_for_shutdown_signal() => {
                signal?;
                futures_util::SinkExt::close(socket).await?;
                return Err(CliError::Interrupted("shutdown requested before registration completed"));
            }
            message = socket.next() => {
                let Some(message) = message else { return Err(CliError::Disconnected("backend disconnected before registration completed")); };
                match message? {
                    Message::Text(text) => {
                        print_backend_message(&text);
                        if let Some(reply) = registration_reply(&text, request_id) {
                            return reply;
                        }
                    }
                    Message::Binary(bytes) => if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                        print_backend_message(&text);
                        if let Some(reply) = registration_reply(&text, request_id) {
                            return reply;
                        }
                    },
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                    Message::Pong(_) => {},
                    Message::Close(_) => return Err(CliError::Disconnected("backend closed before registration completed")),
                    Message::Frame(_) => {},
                }
            }
        }
    }
}

fn registration_reply(text: &str, request_id: &str) -> Option<Result<ClientInfoDto, CliError>> {
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::ConnectionRegistered { id, client }) if id == request_id => {
            Some(Ok(client))
        }
        Ok(ServerMessage::ConnectionRegisterFailed { id, error }) if id == request_id => {
            Some(Err(CliError::Remote(format!(
                "connection registration failed:{}:{}",
                error.kind, error.message
            ))))
        }
        _ => None,
    }
}

fn stop_reply(text: &str, request_id: &str) -> Option<Result<(), CliError>> {
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::WorkspaceStopped { id, .. }) if id == request_id => Some(Ok(())),
        Ok(ServerMessage::WorkspaceStopFailed { id, error }) if id == request_id => {
            Some(Err(CliError::Remote(format!(
                "workspace stop failed:{}:{}",
                error.kind, error.message
            ))))
        }
        _ => None,
    }
}

async fn wait_for_stop_ack(
    socket: &mut (impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
              + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
              + Unpin),
    request_id: &str,
) -> Result<(), CliError> {
    let mut deadline = Box::pin(tokio::time::sleep(Duration::from_secs(5)));
    loop {
        tokio::select! {
            _ = &mut deadline => return Err(CliError::Timeout("timed out waiting for workspace.stopped")),
            signal = wait_for_shutdown_signal() => {
                signal?;
                futures_util::SinkExt::close(socket).await?;
                return Err(CliError::Interrupted("shutdown requested again before workspace stopped"));
            }
            message = socket.next() => {
                let Some(message) = message else { return Err(CliError::Disconnected("backend disconnected before workspace stopped")); };
                match message? {
                    Message::Text(text) => {
                        print_backend_message(&text);
                        if let Some(reply) = stop_reply(&text, request_id) { return reply; }
                    }
                    Message::Binary(bytes) => if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                        print_backend_message(&text);
                        if let Some(reply) = stop_reply(&text, request_id) { return reply; }
                    },
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                    Message::Pong(_) => {},
                    Message::Close(_) => return Err(CliError::Disconnected("backend closed before workspace stopped")),
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

fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("cli-{}-{timestamp}", process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_wait_handles_ping_unrelated_text_and_matching_binary_ack() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket
                .send(Message::Ping(vec![1, 2, 3].into()))
                .await
                .unwrap();
            let pong = socket.next().await.unwrap().unwrap();
            assert!(matches!(pong, Message::Pong(bytes) if bytes.as_ref() == [1, 2, 3]));
            socket
                .send(Message::Text(
                    serde_json::to_string(&ServerMessage::WorkspaceStopFailed {
                        id: "unrelated".into(),
                        error: margatroid_protocol::ProtocolErrorDto::new("Remote", "other"),
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            let stopped = ServerMessage::WorkspaceStopped {
                id: "stop".into(),
                workspace: margatroid_protocol::WorkspaceReferenceDto::with_id(
                    margatroid_protocol::ResourceIdDto("workspace:local/test:latest".into()),
                    "test",
                    "/project",
                ),
            };
            socket
                .send(Message::Binary(
                    serde_json::to_vec(&stopped).unwrap().into(),
                ))
                .await
                .unwrap();
        });
        let (mut socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        wait_for_stop_ack(&mut socket, "stop").await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn registration_disconnect_returns_specific_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket.close(None).await.unwrap();
        });
        let (mut socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        assert!(matches!(
            wait_for_register_ack(&mut socket, "register").await,
            Err(CliError::Disconnected(_))
        ));
        server.await.unwrap();
    }

    #[test]
    fn registration_and_stop_replies_match_only_the_expected_request() {
        let registration = serde_json::to_string(&ServerMessage::ConnectionRegisterFailed {
            id: "registration".into(),
            error: margatroid_protocol::ProtocolErrorDto::new("Remote", "denied"),
        })
        .unwrap();
        assert!(registration_reply(&registration, "another").is_none());
        assert!(matches!(
            registration_reply(&registration, "registration"),
            Some(Err(CliError::Remote(_)))
        ));
        let failed = serde_json::to_string(&ServerMessage::WorkspaceStopFailed {
            id: "stop".into(),
            error: margatroid_protocol::ProtocolErrorDto::new("Remote", "cleanup incomplete"),
        })
        .unwrap();
        assert!(stop_reply(&failed, "another").is_none());
        assert!(matches!(
            stop_reply(&failed, "stop"),
            Some(Err(CliError::Remote(_)))
        ));
        let stopped = serde_json::to_string(&ServerMessage::WorkspaceStopped {
            id: "stop".into(),
            workspace: margatroid_protocol::WorkspaceReferenceDto::with_id(
                margatroid_protocol::ResourceIdDto("workspace:local/test:latest".into()),
                "test",
                "/project",
            ),
        })
        .unwrap();
        assert!(matches!(stop_reply(&stopped, "stop"), Some(Ok(()))));
        assert!(stop_reply("not JSON", "stop").is_none());
    }

    #[test]
    fn defaults_to_workspace_file_and_local_backend() {
        assert_eq!(
            Cli::try_parse_from(["margatroid", "workspace", "up"])
                .unwrap()
                .command,
            Command::Workspace {
                command: WorkspaceCommand::Up(WorkspaceUpArgs {
                    workspace_file: PathBuf::from(DEFAULT_WORKSPACE_FILE),
                    backend_url: DEFAULT_BACKEND_URL.into(),
                }),
            }
        );
    }

    #[test]
    fn accepts_file_and_backend_options() {
        assert_eq!(
            Cli::try_parse_from([
                "margatroid",
                "workspace",
                "up",
                "project/workspace.yaml",
                "--backend",
                "ws://localhost:4000/events",
            ])
            .unwrap()
            .command,
            Command::Workspace {
                command: WorkspaceCommand::Up(WorkspaceUpArgs {
                    workspace_file: PathBuf::from("project/workspace.yaml"),
                    backend_url: "ws://localhost:4000/events".into(),
                }),
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

    #[test]
    fn matching_workspace_start_failure_is_terminal() {
        let message = serde_json::to_string(&ServerMessage::WorkspaceStartFailed {
            id: "start-1".into(),
            error: margatroid_protocol::ProtocolErrorDto::new(
                "ResourceSetupFailed",
                "skill file was not found",
            ),
        })
        .unwrap();

        assert_eq!(
            workspace_start_error(&message, "start-1").as_deref(),
            Some("workspace start failed: ResourceSetupFailed: skill file was not found")
        );
        assert_eq!(workspace_start_error(&message, "other-start"), None);
    }
}
