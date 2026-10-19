mod error;

use error::CliError;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use margatroid_protocol::{ClientInfoDto, ClientMessage, ServerMessage};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const DEFAULT_BACKEND_URL: &str = "ws://127.0.0.1:3939/ws";
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    name = "margatroid",
    version,
    about = "Start registered Margatroid workspaces"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
enum Command {
    /// Start and manage registered workspaces.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
enum WorkspaceCommand {
    /// Select a registered workspace and start it on the backend.
    Up(WorkspaceUpArgs),
}

#[derive(Debug, Args, PartialEq, Eq)]
struct WorkspaceUpArgs {
    /// Registered workspace name (otherwise match the exact current project root).
    #[arg(value_name = "NAME")]
    name: Option<String>,
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
        } => run_workspace_up(arguments.name, arguments.backend_url).await,
    }
}

async fn run_workspace_up(name: Option<String>, backend_url: String) -> Result<(), CliError> {
    let start_request_id = request_id();
    let register_request_id = format!("{start_request_id}-register");
    let registration = serde_json::to_string(&ClientMessage::register_connection(
        register_request_id.as_str(),
        "cli",
    ))?;
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
    let catalog_id = format!("{start_request_id}-catalog");
    socket
        .send(Message::Text(
            serde_json::json!({"type":"workspace.catalog.query", "id":catalog_id, "message":{}})
                .to_string()
                .into(),
        ))
        .await?;
    let entries = wait_for_catalog(&mut socket, &catalog_id).await?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let candidates = catalog_candidates(&entries, name.as_deref(), &cwd);
    let Some(entry) = choose_workspace(&mut socket, candidates, io::stdin().is_terminal()).await?
    else {
        socket.close(None).await?;
        return Ok(());
    };
    if let Some(error) = &entry.error {
        return Err(CliError::Remote(format!(
            "Invalid workspace {}: {error}; fix its registered definition and refresh the catalog",
            entry.name
        )));
    }
    let definition = entry.definition.as_ref().ok_or_else(|| {
        CliError::Remote("Workspace has no valid definition; refresh the catalog".into())
    })?;
    let workspace = margatroid_protocol::WorkspaceReferenceDto::with_id(
        entry.id.clone(),
        entry.name.clone(),
        definition.project_root.clone(),
    );
    let encoded = serde_json::to_string(&ClientMessage::start_workspace(
        &start_request_id,
        entry.id.clone(),
        &entry.revision,
    ))?;
    socket.send(Message::Text(encoded.into())).await?;
    print_cli_event(
        "INFO",
        &format!("workspace.start sent (request_id={start_request_id})"),
    );

    let mut started = false;
    let mut stop_requested = false;
    let mut shutdown = Box::pin(wait_for_shutdown_signal());
    loop {
        if started && stop_requested {
            let stop_id = request_id();
            let stop = ClientMessage::stop_workspace(&stop_id, &workspace);
            socket
                .send(Message::Text(serde_json::to_string(&stop)?.into()))
                .await?;
            print_cli_event(
                "INFO",
                &format!("workspace.stop sent (request_id={stop_id})"),
            );
            wait_for_stop_ack(&mut socket, &stop_id).await?;
            print_cli_event("INFO", &format!("workspace stopped (request_id={stop_id})"));
            socket.close(None).await?;
            print_cli_event("INFO", "backend WebSocket closed");
            return Ok(());
        }
        tokio::select! {
            signal = &mut shutdown => {
                signal?;
                shutdown = Box::pin(wait_for_shutdown_signal());
                stop_requested = true;
                if !started {
                    print_cli_event("INFO", "shutdown requested; waiting for workspace startup result before stopping");
                }
            }
            message = socket.next() => {
                let Some(message) = message else { return Ok(()); };
                match message? {
                    Message::Text(text) => {
                        print_backend_message(&text);
                        handle_workspace_start_reply(&text, &start_request_id, &mut started)?;
                    },
                    Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(text) => {
                            print_backend_message(&text);
                            handle_workspace_start_reply(&text, &start_request_id, &mut started)?;
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

fn catalog_candidates<'a>(
    entries: &'a [margatroid_protocol::WorkspaceCatalogEntryDto],
    name: Option<&str>,
    cwd: &Path,
) -> Vec<&'a margatroid_protocol::WorkspaceCatalogEntryDto> {
    entries
        .iter()
        .filter(|entry| match name {
            Some(name) => entry.name == name,
            None => {
                entry
                    .definition
                    .as_ref()
                    .and_then(|definition| Path::new(&definition.project_root).canonicalize().ok())
                    .as_deref()
                    == Some(cwd)
            }
        })
        .collect()
}
async fn choose_workspace<'a>(
    socket: &mut (impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
              + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
              + Unpin),
    candidates: Vec<&'a margatroid_protocol::WorkspaceCatalogEntryDto>,
    tty: bool,
) -> Result<Option<&'a margatroid_protocol::WorkspaceCatalogEntryDto>, CliError> {
    match candidates.len() {
        0 => return Err(CliError::Remote("No registered workspace matches; use an explicit registered NAME or run from its exact project root".into())),
        1 => return Ok(Some(candidates[0])),
        _ if !tty => return Err(CliError::Remote("Multiple registered workspaces match; specify NAME or use an interactive terminal".into())),
        _ => {}
    }
    for (index, entry) in candidates.iter().enumerate() {
        eprintln!(
            "{}: {} | {} | {}",
            index + 1,
            entry.name,
            entry
                .definition
                .as_ref()
                .map(|d| d.project_root.as_str())
                .unwrap_or(&entry.path),
            entry.lifecycle
        );
    }
    eprintln!("Select a number (blank or q cancels):");
    let (sender, mut input) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = io::stdin().read_line(&mut line).map(|_| line);
        let _ = sender.send(result);
    });
    loop {
        tokio::select! {
            line = &mut input => {
                let line = line.map_err(|_| CliError::Interrupted("workspace chooser closed"))??;
                return parse_choice(line.trim(), &candidates);
            }
            signal = wait_for_shutdown_signal() => { signal?; return Ok(None); }
            message = socket.next() => match message {
                Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await?,
                Some(Ok(Message::Text(text))) => print_backend_message(&text),
                Some(Ok(Message::Binary(bytes))) => if let Ok(text) = String::from_utf8(bytes.to_vec()) { print_backend_message(&text); },
                Some(Ok(Message::Close(_))) | None => return Err(CliError::Disconnected("backend disconnected during selection")),
                Some(Err(error)) => return Err(error.into()),
                _ => {},
            }
        }
    }
}
fn parse_choice<'a>(
    input: &str,
    candidates: &[&'a margatroid_protocol::WorkspaceCatalogEntryDto],
) -> Result<Option<&'a margatroid_protocol::WorkspaceCatalogEntryDto>, CliError> {
    if input.is_empty() || input.eq_ignore_ascii_case("q") {
        return Ok(None);
    }
    input
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|n| candidates.get(n).copied())
        .map(Some)
        .ok_or_else(|| CliError::Remote("Invalid workspace selection; nothing started".into()))
}
async fn wait_for_catalog(
    socket: &mut (impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
              + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
              + Unpin),
    request_id: &str,
) -> Result<Vec<margatroid_protocol::WorkspaceCatalogEntryDto>, CliError> {
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return Err(CliError::Timeout("timed out waiting for workspace.catalog")),
            signal = wait_for_shutdown_signal() => { signal?; return Err(CliError::Interrupted("shutdown requested before selection")); },
            message = socket.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Binary(bytes))) => String::from_utf8_lossy(&bytes).into_owned(),
                    Some(Ok(Message::Ping(payload))) => { socket.send(Message::Pong(payload)).await?; continue; },
                    Some(Ok(Message::Close(_))) | None => return Err(CliError::Disconnected("backend disconnected before catalog reply")),
                    Some(Err(error)) => return Err(error.into()),
                    _ => continue,
                };
                print_backend_message(&text);
                match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(ServerMessage::WorkspaceCatalog { id, entries }) if id == request_id => return Ok(entries),
                    Ok(ServerMessage::RequestFailed { id, error }) if id == request_id => return Err(CliError::Remote(error.message)),
                    _ => {},
                }
            }
        }
    }
}

fn handle_workspace_start_reply(
    text: &str,
    request_id: &str,
    started: &mut bool,
) -> Result<(), CliError> {
    if *started {
        return Ok(());
    }
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::WorkspaceStarted { id, workspace }) if id == request_id => {
            *started = true;
            print_cli_event(
                "INFO",
                &format!(
                    "workspace started (workspace={}, member_failures={})",
                    workspace.id.0,
                    workspace.member_failures.len()
                ),
            );
            for failure in workspace.member_failures {
                print_cli_event(
                    "ERROR",
                    &format!(
                        "workspace member failed to start (workspace={}, agent={}, driver={}, cause={})",
                        workspace.id.0, failure.agent.0, failure.driver.0, failure.error
                    ),
                );
            }
            Ok(())
        }
        Ok(ServerMessage::WorkspaceStartFailed { id, error })
        | Ok(ServerMessage::RequestFailed { id, error })
            if id == request_id =>
        {
            Err(CliError::Remote(format!(
                "workspace start failed: {}: {}",
                error.kind, error.message
            )))
        }
        _ => Ok(()),
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

    fn catalog_entry(name: &str, root: &Path) -> margatroid_protocol::WorkspaceCatalogEntryDto {
        serde_json::from_value(serde_json::json!({
            "id": format!("workspace:local/{name}:latest"), "name": name,
            "path": format!("/catalog/{name}/workspace.yaml"), "revision": "revision-1", "lifecycle": "stopped",
            "definition": {"id": format!("workspace:local/{name}:latest"), "name":name, "project_root":root, "agents":[]}
        })).unwrap()
    }

    #[test]
    fn catalog_selection_is_exact_canonical_root_or_explicit_name() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let entries = vec![
            catalog_entry("one", root.path()),
            catalog_entry("two", root.path()),
        ];
        assert_eq!(
            catalog_candidates(&entries, None, &root.path().canonicalize().unwrap()).len(),
            2
        );
        assert!(catalog_candidates(&entries, None, &nested.canonicalize().unwrap()).is_empty());
        assert_eq!(catalog_candidates(&entries, Some("two"), &nested).len(), 1);
        assert!(
            catalog_candidates(&entries, Some("/arbitrary/workspace.yaml"), root.path()).is_empty()
        );
        assert!(catalog_candidates(&entries, Some("unknown"), root.path()).is_empty());
        #[cfg(unix)]
        {
            let link = root.path().join("alias");
            std::os::unix::fs::symlink(root.path(), &link).unwrap();
            let alias_entries = vec![catalog_entry("alias", &link)];
            assert_eq!(
                catalog_candidates(&alias_entries, None, &root.path().canonicalize().unwrap())
                    .len(),
                1
            );
        }
    }

    #[test]
    fn chooser_number_cancel_and_invalid_input_never_guess() {
        let root = tempfile::tempdir().unwrap();
        let entries = [
            catalog_entry("one", root.path()),
            catalog_entry("two", root.path()),
        ];
        let choices = entries.iter().collect::<Vec<_>>();
        assert_eq!(parse_choice("2", &choices).unwrap().unwrap().name, "two");
        for cancel in ["", "q", "Q"] {
            assert!(parse_choice(cancel, &choices).unwrap().is_none());
        }
        for invalid in ["0", "3", "one", "-1"] {
            assert!(parse_choice(invalid, &choices).is_err());
        }
    }

    #[tokio::test]
    async fn nonterminal_ambiguous_or_missing_selection_does_not_send_start() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream).await.unwrap()
        });
        let (mut socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        let _peer = server.await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let entries = [
            catalog_entry("one", root.path()),
            catalog_entry("two", root.path()),
        ];
        assert!(
            choose_workspace(&mut socket, entries.iter().collect(), false)
                .await
                .is_err()
        );
        assert!(choose_workspace(&mut socket, vec![], false).await.is_err());
    }

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
    fn defaults_to_exact_cwd_selection_and_local_backend() {
        assert_eq!(
            Cli::try_parse_from(["margatroid", "workspace", "up"])
                .unwrap()
                .command,
            Command::Workspace {
                command: WorkspaceCommand::Up(WorkspaceUpArgs {
                    name: None,
                    backend_url: DEFAULT_BACKEND_URL.into(),
                }),
            }
        );
    }

    #[test]
    fn accepts_name_and_backend_options() {
        assert_eq!(
            Cli::try_parse_from([
                "margatroid",
                "workspace",
                "up",
                "registered-project",
                "--backend",
                "ws://localhost:4000/events",
            ])
            .unwrap()
            .command,
            Command::Workspace {
                command: WorkspaceCommand::Up(WorkspaceUpArgs {
                    name: Some("registered-project".into()),
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
    fn workspace_success_with_any_number_of_member_failures_is_nonterminal() {
        for failures in [
            serde_json::json!([]),
            serde_json::json!([
                {"agent":"agent:test/coder:latest", "driver":"driver:local/missing:latest", "error":"missing coder"},
                {"agent":"agent:test/reviewer:latest", "driver":"driver:local/missing:latest", "error":"missing reviewer"}
            ]),
        ] {
            let text = serde_json::json!({
                "type":"workspace.started", "id":"start",
                "workspace":{"id":"workspace:local/test:latest", "name":"test", "project_root":"/project",
                    "agents":[], "member_failures":failures}
            }).to_string();
            let mut started = false;
            handle_workspace_start_reply(&text, "other", &mut started).unwrap();
            assert!(!started);
            handle_workspace_start_reply(&text, "start", &mut started).unwrap();
            assert!(started);
        }
        let mut started = false;
        handle_workspace_start_reply("not JSON", "start", &mut started).unwrap();
        assert!(!started);
    }

    #[test]
    fn matching_start_request_rejection_is_terminal_before_success_only() {
        let text = serde_json::to_string(&ServerMessage::RequestFailed {
            id: "start".into(),
            error: margatroid_protocol::ProtocolErrorDto::new(
                "InvalidRequest",
                "workspace definition missing before creation",
            ),
        })
        .unwrap();
        let mut started = false;
        handle_workspace_start_reply(&text, "other", &mut started).unwrap();
        assert!(handle_workspace_start_reply(&text, "start", &mut started).is_err());
        started = true;
        handle_workspace_start_reply(&text, "start", &mut started).unwrap();
    }

    #[test]
    fn matching_workspace_start_failure_is_terminal() {
        let message = serde_json::to_string(&ServerMessage::WorkspaceStartFailed {
            id: "start-1".into(),
            error: margatroid_protocol::ProtocolErrorDto::new(
                "ExecutionStartFailed",
                "workspace execution thread could not start",
            ),
        })
        .unwrap();

        let mut started = false;
        let error = handle_workspace_start_reply(&message, "start-1", &mut started).unwrap_err();
        assert!(error.to_string().contains("ExecutionStartFailed"));
        assert!(error
            .to_string()
            .contains("workspace execution thread could not start"));
        assert!(!started);
        handle_workspace_start_reply(&message, "other-start", &mut started).unwrap();
        started = true;
        handle_workspace_start_reply(&message, "start-1", &mut started).unwrap();
    }
}
