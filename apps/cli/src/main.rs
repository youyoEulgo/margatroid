mod error;

use clap::{Args, Parser, Subcommand};
use error::CliError;
use futures_util::{SinkExt, StreamExt};
use margatroid_protocol::{
    ClientInfoDto, ClientMessage, EmptyDto, RegisterConnectionDto, ServerMessage,
    WorkspaceConfigDto, WorkspacePathDto, WorkspaceReferenceDto, WorkspaceStopDto,
};
use std::{
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    process,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const DEFAULT_BACKEND_URL: &str = "ws://127.0.0.1:3939/ws";
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    name = "margatroid",
    version,
    about = "Start configured Margatroid workspaces"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Debug, Subcommand, PartialEq, Eq)]
enum Command {
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
}
#[derive(Debug, Subcommand, PartialEq, Eq)]
enum WorkspaceCommand {
    Up(WorkspaceUpArgs),
}
#[derive(Debug, Args, PartialEq, Eq)]
struct WorkspaceUpArgs {
    #[arg(
        value_name = "NAME",
        help = "Workspace display name; otherwise match the exact current project root"
    )]
    name: Option<String>,
    #[arg(long="backend",value_name="WS_URL",default_value=DEFAULT_BACKEND_URL)]
    backend_url: String,
}
#[derive(Default)]
struct CliSession {
    pending_id: Option<String>,
    workspace_path: Option<PathBuf>,
    workspace_address: Option<String>,
    stop_requested: bool,
}
#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse().command).await {
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
async fn send(socket: &mut Socket, message: ClientMessage) -> Result<(), CliError> {
    socket
        .send(Message::Text(serde_json::to_string(&message)?.into()))
        .await?;
    Ok(())
}
async fn next_text(socket: &mut Socket, disconnected: &'static str) -> Result<String, CliError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return Ok(text.to_string()),
            Some(Ok(Message::Binary(bytes))) => match String::from_utf8(bytes.to_vec()) {
                Ok(text) => return Ok(text),
                Err(_) => print_cli_event("WARN", "non-UTF-8 backend message ignored"),
            },
            Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await?,
            Some(Ok(Message::Pong(_) | Message::Frame(_))) => {}
            Some(Ok(Message::Close(_))) | None => return Err(CliError::Disconnected(disconnected)),
            Some(Err(error)) => return Err(error.into()),
        }
    }
}
async fn run_workspace_up(name: Option<String>, backend_url: String) -> Result<(), CliError> {
    let start_id = request_id();
    let register_id = format!("{start_id}-register");
    let mut session = CliSession::default();
    print_cli_event(
        "INFO",
        &format!("connecting to backend (url={backend_url})"),
    );
    let (mut socket, _) = connect_async(&backend_url).await?;
    send(
        &mut socket,
        ClientMessage::ConnectionRegister {
            id: register_id.clone(),
            message: RegisterConnectionDto {
                client_type: "cli".into(),
                name: None,
            },
        },
    )
    .await?;
    let registered = wait_for_register_ack(&mut socket, &register_id).await?;
    print_cli_event(
        "INFO",
        &format!(
            "connection registered (client={}, type={}, name={})",
            registered.resource_id, registered.client_type, registered.name
        ),
    );
    let state_id = format!("{start_id}-state");
    session.pending_id = Some(state_id.clone());
    send(
        &mut socket,
        ClientMessage::StateQuery {
            id: state_id.clone(),
            message: EmptyDto::default(),
        },
    )
    .await?;
    let definitions = wait_for_state(&mut socket, &state_id).await?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let candidates = definition_candidates(&definitions, name.as_deref(), &cwd);
    let Some(definition) =
        choose_workspace(&mut socket, candidates, io::stdin().is_terminal()).await?
    else {
        socket.close(None).await?;
        return Ok(());
    };
    session.workspace_path = Some(definition.path.clone().into());
    session.pending_id = Some(start_id.clone());
    send(
        &mut socket,
        ClientMessage::WorkspaceStart {
            id: start_id.clone(),
            message: WorkspacePathDto {
                path: definition.path.clone(),
            },
        },
    )
    .await?;
    print_cli_event(
        "INFO",
        &format!(
            "workspace.start sent (path={}, request_id={start_id})",
            definition.path
        ),
    );
    let mut shutdown = Box::pin(wait_for_shutdown_signal());
    loop {
        if session.stop_requested {
            if let Some(address) = &session.workspace_address {
                let stop_id = request_id();
                session.pending_id = Some(stop_id.clone());
                send(
                    &mut socket,
                    ClientMessage::WorkspaceStop {
                        id: stop_id.clone(),
                        message: WorkspaceStopDto {
                            workspace: WorkspaceReferenceDto::Address(address.clone()),
                        },
                    },
                )
                .await?;
                print_cli_event(
                    "INFO",
                    &format!("workspace.stop sent (workspace={address}, request_id={stop_id})"),
                );
                wait_for_stop_ack(&mut socket, &stop_id).await?;
                print_cli_event("INFO", &format!("workspace stopped (request_id={stop_id})"));
                socket.close(None).await?;
                return Ok(());
            }
        }
        tokio::select! {
            signal=&mut shutdown=>{signal?;shutdown=Box::pin(wait_for_shutdown_signal());session.stop_requested=true;if session.workspace_address.is_none(){print_cli_event("INFO","shutdown requested; waiting for workspace startup result before stopping");}},
            text=next_text(&mut socket,"backend disconnected before confirmed workspace shutdown")=>{let text=text?;print_backend_message(&text);handle_workspace_start_reply(&text,&start_id,&mut session)?;}
        }
    }
}
fn definition_candidates<'a>(
    definitions: &'a [WorkspaceConfigDto],
    name: Option<&str>,
    cwd: &Path,
) -> Vec<&'a WorkspaceConfigDto> {
    definitions
        .iter()
        .filter(|definition| match name {
            Some(name) => definition.cache.name == name,
            None => {
                Path::new(&definition.cache.project_root)
                    .canonicalize()
                    .ok()
                    .as_deref()
                    == Some(cwd)
            }
        })
        .collect()
}
async fn choose_workspace<'a>(
    socket: &mut Socket,
    candidates: Vec<&'a WorkspaceConfigDto>,
    tty: bool,
) -> Result<Option<&'a WorkspaceConfigDto>, CliError> {
    match candidates.len(){0=>return Err(CliError::Remote("No configured workspace matches; specify its exact NAME or run from its exact project root".into())),1=>return Ok(Some(candidates[0])),_ if !tty=>return Err(CliError::Remote("Multiple configured workspaces match; specify NAME or use an interactive terminal".into())),_=>{}}
    for (index, definition) in candidates.iter().enumerate() {
        eprintln!(
            "{}: {} | {} | {}",
            index + 1,
            definition.cache.name,
            definition.cache.project_root,
            definition.path
        );
    }
    eprintln!("Select a number (blank or q cancels):");
    let (sender, mut receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = io::stdin().read_line(&mut line).map(|_| line);
        let _ = sender.send(result);
    });
    loop {
        tokio::select! {
            line=&mut receiver=>{let line=line.map_err(|_|CliError::Interrupted("workspace chooser closed"))??;return parse_choice(line.trim(),&candidates);},
            signal=wait_for_shutdown_signal()=>{signal?;return Ok(None);},
            text=next_text(socket,"backend disconnected during selection")=>print_backend_message(&text?),
        }
    }
}
fn parse_choice<'a>(
    input: &str,
    candidates: &[&'a WorkspaceConfigDto],
) -> Result<Option<&'a WorkspaceConfigDto>, CliError> {
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
async fn wait_for_state(
    socket: &mut Socket,
    request_id: &str,
) -> Result<Vec<WorkspaceConfigDto>, CliError> {
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _=&mut deadline=>return Err(CliError::Timeout("timed out waiting for state.sync")),
            signal=wait_for_shutdown_signal()=>{signal?;return Err(CliError::Interrupted("shutdown requested before selection"));},
            text=next_text(socket,"backend disconnected before state.sync")=>{
                let text=text?;print_backend_message(&text);match serde_json::from_str::<ServerMessage>(&text){Ok(ServerMessage::StateSync{state})=>return Ok(state.workspace_definitions),Ok(ServerMessage::RequestFailed{id,error})if id==request_id=>return Err(CliError::Remote(error.message)),_=>{}}
            }
        }
    }
}
fn handle_workspace_start_reply(
    text: &str,
    request_id: &str,
    session: &mut CliSession,
) -> Result<(), CliError> {
    if session.workspace_address.is_some() {
        return Ok(());
    }
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::WorkspaceStarted { id, workspace }) if id == request_id => {
            session.workspace_address = Some(workspace.id.clone());
            session.pending_id = None;
            print_cli_event(
                "INFO",
                &format!(
                    "workspace started (workspace={}, member_failures={})",
                    workspace.id,
                    workspace.member_failures.len()
                ),
            );
            for failure in workspace.member_failures {
                print_cli_event("ERROR",&format!("workspace member failed to start (workspace={}, agent={}, driver={}, cause={})",workspace.id,failure.agent,failure.driver,failure.error));
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
    socket: &mut Socket,
    request_id: &str,
) -> Result<ClientInfoDto, CliError> {
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _=&mut deadline=>return Err(CliError::Timeout("timed out waiting for connection.registered")),
            signal=wait_for_shutdown_signal()=>{signal?;socket.close(None).await?;return Err(CliError::Interrupted("shutdown requested before registration completed"));},
            text=next_text(socket,"backend disconnected before registration completed")=>{let text=text?;print_backend_message(&text);if let Some(result)=registration_reply(&text,request_id){return result;}}
        }
    }
}
fn registration_reply(text: &str, request_id: &str) -> Option<Result<ClientInfoDto, CliError>> {
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::ConnectionRegistered { id, client }) if id == request_id => {
            Some(Ok(client))
        }
        Ok(ServerMessage::ConnectionRegisterFailed { id, error })
        | Ok(ServerMessage::RequestFailed { id, error })
            if id == request_id =>
        {
            Some(Err(CliError::Remote(format!(
                "connection registration failed: {}: {}",
                error.kind, error.message
            ))))
        }
        _ => None,
    }
}
fn stop_reply(text: &str, request_id: &str) -> Option<Result<(), CliError>> {
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::WorkspaceStopped { id, .. }) if id == request_id => Some(Ok(())),
        Ok(ServerMessage::WorkspaceStopFailed { id, error })
        | Ok(ServerMessage::RequestFailed { id, error })
            if id == request_id =>
        {
            Some(Err(CliError::Remote(format!(
                "workspace stop failed: {}: {}",
                error.kind, error.message
            ))))
        }
        _ => None,
    }
}
async fn wait_for_stop_ack(socket: &mut Socket, request_id: &str) -> Result<(), CliError> {
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _=&mut deadline=>return Err(CliError::Timeout("timed out waiting for workspace.stopped")),
            signal=wait_for_shutdown_signal()=>{signal?;socket.close(None).await?;return Err(CliError::Interrupted("shutdown requested again before workspace stopped"));},
            text=next_text(socket,"backend disconnected before workspace stopped")=>{let text=text?;print_backend_message(&text);if let Some(result)=stop_reply(&text,request_id){return result;}}
        }
    }
}
async fn wait_for_shutdown_signal() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {result=tokio::signal::ctrl_c()=>result,_=terminate.recv()=>Ok(())}
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
            io::stdout().is_terminal()
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
            io::stderr().is_terminal()
        )
    );
}
fn format_log_line(
    timestamp: u64,
    level: &str,
    target: &str,
    message: &str,
    fields: &str,
    ansi: bool,
) -> String {
    let timestamp = format_timestamp(timestamp);
    if !ansi {
        return format!("{timestamp} {level:<5} {target}: {message}{fields}");
    }
    let color = match level {
        "ERROR" => "\x1b[31m",
        "WARN" => "\x1b[33m",
        "INFO" => "\x1b[32m",
        "DEBUG" => "\x1b[34m",
        "TRACE" => "\x1b[35m",
        _ => ANSI_RESET,
    };
    format!("{ANSI_DIM}{timestamp}{ANSI_RESET} {color}{level:<5}{ANSI_RESET} {ANSI_DIM}{target}:{ANSI_RESET} {message}{fields}")
}
fn format_timestamp(timestamp: u64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp).saturating_mul(1_000_000))
        .ok()
        .and_then(|timestamp| timestamp.format(&Rfc3339).ok())
        .unwrap_or_else(|| timestamp.to_string())
}
fn request_id() -> String {
    format!(
        "cli-{}-{}",
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
#[cfg(test)]
#[path = "../tests/support/unit.rs"]
mod tests;
