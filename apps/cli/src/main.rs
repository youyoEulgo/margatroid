use std::error::Error;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use compose::compile;
use futures_util::{SinkExt, StreamExt};
use margatroid_protocol::{ClientMessage, ServerMessage};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const DEFAULT_WORKSPACE_FILE: &str = "margatroid-workspace.yaml";
const DEFAULT_BACKEND_URL: &str = "ws://127.0.0.1:3939/ws";

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
                eprintln!("margatroid: {error}");
                process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("margatroid: {error}");
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
    let request_id = request_id();
    let registration = serde_json::to_string(&ClientMessage::register_connection(
        format!("{request_id}-register"),
        "cli",
    ))?;
    let request = ClientMessage::start_workspace(&request_id, &definition);
    let encoded = serde_json::to_string(&request)?;

    eprintln!(
        "compiled {} (workspace '{}')",
        workspace_file.display(),
        definition.name
    );
    eprintln!("connecting to {backend_url}");
    let (mut socket, _) = connect_async(&backend_url).await?;
    socket.send(Message::Text(registration.into())).await?;
    socket.send(Message::Text(encoded.into())).await?;
    eprintln!("workspace.start sent (request '{request_id}'); waiting for backend logs");

    while let Some(message) = socket.next().await {
        match message? {
            Message::Text(text) => print_backend_message(&text),
            Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                Ok(text) => print_backend_message(&text),
                Err(_) => eprintln!("[backend binary message: {} bytes]", bytes.len()),
            },
            Message::Ping(payload) => {
                socket.send(Message::Pong(payload)).await?;
            }
            Message::Pong(_) => {}
            Message::Close(frame) => {
                if let Some(frame) = frame {
                    eprintln!(
                        "backend closed WebSocket ({}): {}",
                        frame.code, frame.reason
                    );
                } else {
                    eprintln!("backend closed WebSocket");
                }
                break;
            }
            Message::Frame(_) => {}
        }
    }
    Ok(())
}

fn print_backend_message(text: &str) {
    let Ok(ServerMessage::Log { record }) = serde_json::from_str(text) else {
        return;
    };
    let fields = record
        .fields
        .iter()
        .map(|field| format!("{}={}", field.name, field.value))
        .collect::<Vec<_>>();
    let fields = if fields.is_empty() {
        String::new()
    } else {
        format!(" ({})", fields.join(", "))
    };
    println!(
        "{} {} {}: {}{}",
        record.timestamp_millis, record.level, record.target, record.message, fields
    );
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
}
