mod error;

use clap::{Args, Parser, Subcommand};
use error::CliError;
use futures_util::{SinkExt, StreamExt};
use margatroid_protocol::{
    ClientInfoDto, ClientMessage, EmptyDto, RegisterConnectionDto, ServerMessage, WorkspaceInfoDto,
    WorkspacePathDto,
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
    workspace_started: bool,
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
            "connection registered (client={}, type={})",
            registered.address, registered.client_type
        ),
    );
    let state_id = format!("{start_id}-state");
    session.pending_id = Some(state_id.clone());
    send(
        &mut socket,
        ClientMessage::WorkspaceReq {
            id: state_id.clone(),
            message: EmptyDto {},
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
        if session.stop_requested && session.workspace_started {
            let path = session
                .workspace_path
                .as_ref()
                .expect("selected workspace")
                .to_string_lossy()
                .into_owned();
            let stop_id = request_id();
            session.pending_id = Some(stop_id.clone());
            send(
                &mut socket,
                ClientMessage::WorkspaceStop {
                    id: stop_id.clone(),
                    message: WorkspacePathDto { path: path.clone() },
                },
            )
            .await?;
            print_cli_event(
                "INFO",
                &format!("workspace.stop sent (path={path}, request_id={stop_id})"),
            );
            wait_for_stop_ack(&mut socket, &stop_id).await?;
            print_cli_event("INFO", &format!("workspace stopped (request_id={stop_id})"));
            socket.close(None).await?;
            return Ok(());
        }
        tokio::select! {
            signal=&mut shutdown=>{signal?;shutdown=Box::pin(wait_for_shutdown_signal());session.stop_requested=true;if !session.workspace_started{print_cli_event("INFO","shutdown requested; waiting for workspace startup result before stopping");}},
            text=next_text(&mut socket,"backend disconnected before confirmed workspace shutdown")=>{let text=text?;print_backend_message(&text);handle_workspace_start_reply(&text,&start_id,&mut session)?;}
        }
    }
}
fn definition_candidates<'a>(
    definitions: &'a [WorkspaceInfoDto],
    name: Option<&str>,
    cwd: &Path,
) -> Vec<&'a WorkspaceInfoDto> {
    definitions
        .iter()
        .filter(|definition| match name {
            Some(name) => definition.definition.name == name,
            None => {
                Path::new(&definition.definition.project_root)
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
    candidates: Vec<&'a WorkspaceInfoDto>,
    tty: bool,
) -> Result<Option<&'a WorkspaceInfoDto>, CliError> {
    match candidates.len(){0=>return Err(CliError::Remote("No configured workspace matches; specify its exact NAME or run from its exact project root".into())),1=>return Ok(Some(candidates[0])),_ if !tty=>return Err(CliError::Remote("Multiple configured workspaces match; specify NAME or use an interactive terminal".into())),_=>{}}
    for (index, definition) in candidates.iter().enumerate() {
        eprintln!(
            "{}: {} | {} | {}",
            index + 1,
            definition.definition.name,
            definition.definition.project_root,
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
    candidates: &[&'a WorkspaceInfoDto],
) -> Result<Option<&'a WorkspaceInfoDto>, CliError> {
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
) -> Result<Vec<WorkspaceInfoDto>, CliError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let text = next_text(socket, "backend disconnected before workspace.res").await?;
            if let Ok(ServerMessage::WorkspaceRes {
                id,
                success,
                workspaces,
                error,
            }) = serde_json::from_str(&text)
            {
                if id == request_id {
                    return if success {
                        Ok(workspaces.unwrap_or_default())
                    } else {
                        Err(CliError::Remote(
                            error
                                .map(|e| e.message)
                                .unwrap_or_else(|| "workspace query failed".into()),
                        ))
                    };
                }
            }
        }
    })
    .await
    .map_err(|_| CliError::Timeout("timed out waiting for workspace.res"))?
}
fn handle_workspace_start_reply(
    text: &str,
    request_id: &str,
    session: &mut CliSession,
) -> Result<(), CliError> {
    if session.workspace_started {
        return Ok(());
    }
    if let Ok(ServerMessage::WorkspaceStartResult { id, success, error }) =
        serde_json::from_str(text)
    {
        if id == request_id {
            if let Some(e) = error {
                return Err(CliError::Remote(format!(
                    "workspace start failed: {}: {}",
                    e.code, e.message
                )));
            }
            if success {
                session.workspace_started = true;
                session.pending_id = None;
                print_cli_event("INFO", "workspace started");
            }
        }
    }
    Ok(())
}
async fn wait_for_register_ack(
    socket: &mut Socket,
    request_id: &str,
) -> Result<ClientInfoDto, CliError> {
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _=&mut deadline=>return Err(CliError::Timeout("timed out waiting for connection.register.result")),
            signal=wait_for_shutdown_signal()=>{signal?;socket.close(None).await?;return Err(CliError::Interrupted("shutdown requested before registration completed"));},
            text=next_text(socket,"backend disconnected before registration completed")=>{let text=text?;print_backend_message(&text);if let Some(result)=registration_reply(&text,request_id){return result;}}
        }
    }
}
fn registration_reply(text: &str, request_id: &str) -> Option<Result<ClientInfoDto, CliError>> {
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::ConnectionRegisterResult {
            id,
            success,
            client,
            error,
        }) if id == request_id => Some(if success {
            client.ok_or_else(|| CliError::Remote("registration identity missing".into()))
        } else {
            Err(CliError::Remote(
                error
                    .map(|e| format!("{}: {}", e.code, e.message))
                    .unwrap_or_else(|| "registration failed".into()),
            ))
        }),
        _ => None,
    }
}
fn stop_reply(text: &str, request_id: &str) -> Option<Result<(), CliError>> {
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::WorkspaceStopResult { id, success, error }) if id == request_id => {
            Some(if success {
                Ok(())
            } else {
                Err(CliError::Remote(
                    error
                        .map(|e| format!("{}: {}", e.code, e.message))
                        .unwrap_or_else(|| "stop failed".into()),
                ))
            })
        }
        _ => None,
    }
}
async fn wait_for_stop_ack(socket: &mut Socket, request_id: &str) -> Result<(), CliError> {
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _=&mut deadline=>return Err(CliError::Timeout("timed out waiting for workspace.stop.result")),
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
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(ServerMessage::AgentLogSync { address, records }) => {
            for record in records {
                println!(
                    "{}",
                    format_log_line(
                        record.created_at_ms,
                        &record.level,
                        &address,
                        &record.message,
                        "",
                        io::stdout().is_terminal()
                    )
                );
            }
        }
        Ok(ServerMessage::AgentSync { agents }) => {
            for agent in agents {
                if let Some(error) = agent.error {
                    print_cli_event(
                        "ERROR",
                        &format!("{}: {}: {}", agent.address, error.code, error.message),
                    );
                }
            }
        }
        _ => (),
    }
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
