use std::error::Error;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process;

use agent_image_loader_plugin::AgentImageLoaderPlugin;
use agent_plugin::AgentPlugin;
use app_runtime_plugin::{AppRunExt, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{AsyncRuntimePlugin, WorldAsyncExt};
use core_plugin::{App, Event, World};
use inference_plugin::InferencePlugin;
use log_plugin::{LogPlugin, TracingRecord, TracingStream, TracingStreamError};
use margatroid_protocol::{
    AgentFailureDto, AgentMessageDto, ClientRequest, LogFieldDto, LogRecordDto, ServerEvent,
    WorkspaceInfoDto,
};
use margatroid_types::{AgentFailure, AgentMessage as DomainAgentMessage, Message, MessageIntent};
use memory_plugin::MemoryPlugin;
use server_plugin::{
    AppServerExt, ServerFailed, ServerOptions, ServerPlugin, ServerStarted, ServerStopped,
    WebSocketConnections, WebSocketMessage, WebSocketMessageReceived,
};
use tool_plugin::ToolPlugin;
use tracing::info;
use workspace_plugin::{
    StartWorkspace, StartWorkspaceResult, WorkspaceAgents, WorkspaceConfiguration, WorkspacePlugin,
    WorldWorkspaceExt,
};

const DEFAULT_BIND: &str = "127.0.0.1:3939";
const DEFAULT_DATA_ROOT: &str = ".margatroid";
const LOG_STREAM_CAPACITY: usize = 256;

struct DaemonStart;

impl Event for DaemonStart {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DaemonConfig {
    bind: SocketAddr,
    data_root: PathBuf,
}

fn main() {
    match parse_args(std::env::args().skip(1)) {
        Ok(config) => {
            if let Err(error) = run(config) {
                eprintln!("margatroid-daemon: {error}");
                process::exit(1);
            }
        }
        Err(error) if error == usage() => println!("{}", usage()),
        Err(error) => {
            eprintln!("margatroid-daemon: {error}");
            eprintln!();
            eprintln!("{}", usage());
            process::exit(2);
        }
    }
}

fn run(config: DaemonConfig) -> Result<(), Box<dyn Error + Send + Sync>> {
    fs::create_dir_all(&config.data_root)?;
    let data_root = absolute_path(&config.data_root)?;
    let agent_images_root = data_root.join("agent-images");
    let models_path = data_root.join("models.toml");
    if !models_path.is_file() {
        return Err(format!(
            "model route configuration is missing: {}",
            models_path.display()
        )
        .into());
    }
    let agent_images = AgentImageLoaderPlugin::open(&agent_images_root)
        .map_err(|error| format!("cannot open agent image root: {error}"))?;
    let workspace = WorkspacePlugin::open(&agent_images_root)
        .map_err(|error| format!("cannot open workspace agent image root: {error}"))?;

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(LogPlugin::default().with_stream(LOG_STREAM_CAPACITY))
        .add_plugin(ServerPlugin::with_options(ServerOptions::bind(config.bind)))
        .add_plugin(agent_images)
        .add_plugin(InferencePlugin::default().with_config_path(models_path.clone()))
        .add_plugin(ToolPlugin::default())
        .add_plugin(MemoryPlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(workspace)
        .add_websocket_event_route("/ws")
        .add_system(RuntimePlugin::UPDATE, handle_websocket_messages)
        .add_system(RuntimePlugin::UPDATE, report_runtime_events);

    install_log_forwarder(&app)?;
    app.world().send_event(DaemonStart);
    info!(address = %config.bind, data_root = %data_root.display(), models = %models_path.display(), "margatroid daemon starting");
    app.run();
    Ok(())
}

fn install_log_forwarder(app: &App) -> Result<(), Box<dyn Error + Send + Sync>> {
    let stream = app
        .world()
        .get_resource::<TracingStream>()
        .cloned()
        .ok_or("LogPlugin stream is not available")?;
    let connections = app
        .world()
        .get_resource::<WebSocketConnections>()
        .cloned()
        .ok_or("ServerPlugin WebSocket registry is not available")?;
    app.world()
        .spawn_async_service(forward_logs(stream, connections));
    Ok(())
}

async fn forward_logs(stream: TracingStream, connections: WebSocketConnections) {
    let mut subscription = stream.subscribe();
    loop {
        let record = match subscription.recv().await {
            Ok(record) => record,
            Err(TracingStreamError::Lagged(count)) => {
                tracing::warn!(dropped = count, "daemon log stream lagged");
                continue;
            }
            Err(TracingStreamError::Closed) => break,
            Err(_) => break,
        };
        let event = ServerEvent::Log {
            record: log_record(record),
        };
        let Ok(encoded) = serde_json::to_string(&event) else {
            continue;
        };
        for sender in connections.unnamed() {
            let _ = sender
                .send(WebSocketMessage::Text(encoded.clone().into()))
                .await;
        }
    }
}

fn log_record(record: TracingRecord) -> LogRecordDto {
    LogRecordDto {
        timestamp_millis: record.timestamp_millis,
        level: record.level,
        target: record.target,
        message: record.message,
        fields: record
            .fields
            .into_iter()
            .map(|field| LogFieldDto {
                name: field.name,
                value: field.value,
            })
            .collect(),
        spans: record.spans,
    }
}

fn handle_websocket_messages(world: &mut World) {
    let messages = world
        .event_reader::<WebSocketMessageReceived>()
        .into_iter()
        .collect::<Vec<_>>();
    for received in messages {
        let WebSocketMessage::Text(text) = &received.message else {
            tracing::warn!(
                connection = received.connection_id.get(),
                "ignoring non-text WebSocket message"
            );
            continue;
        };
        let request = match serde_json::from_str::<ClientRequest>(text.as_str()) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(
                    connection = received.connection_id.get(),
                    error = %error,
                    "ignoring invalid daemon request"
                );
                continue;
            }
        };
        match request {
            ClientRequest::WorkspaceStart { id, definition } => {
                match definition.into_definition() {
                    Ok(definition) => world.send_event(StartWorkspace { id, definition }),
                    Err(error) => tracing::warn!(
                        connection = received.connection_id.get(),
                        error = %error,
                        "workspace request contains an invalid definition"
                    ),
                }
            }
            ClientRequest::AgentMessage {
                id,
                workspace,
                agent,
                content,
            } => {
                if let Err(error) = route_agent_message(world, id, workspace, agent, content) {
                    tracing::warn!(
                        connection = received.connection_id.get(),
                        error = %error,
                        "agent message was rejected"
                    );
                }
            }
        }
    }
}

fn report_runtime_events(world: &mut World) {
    let Some(connections) = world.get_resource::<WebSocketConnections>().cloned() else {
        return;
    };
    for event in world.event_reader::<ServerStarted>() {
        tracing::info!(address = %event.address, "daemon WebSocket server started");
    }
    for event in world.event_reader::<ServerFailed>() {
        tracing::error!(error = %event.message, "daemon WebSocket server failed");
    }
    if !world.event_reader::<ServerStopped>().is_empty() {
        tracing::info!("daemon WebSocket server stopped");
    }
    for result in world.event_reader::<StartWorkspaceResult>() {
        match &result.result {
            Ok(workspace) => {
                tracing::info!(request_id = %result.id, "workspace started");
                if let Some(info) = workspace_info(world, *workspace) {
                    broadcast_server_event(
                        &connections,
                        &ServerEvent::WorkspaceStarted {
                            id: result.id.clone(),
                            workspace: info,
                        },
                    );
                }
            }
            Err(error) => {
                tracing::error!(request_id = %result.id, error = %error, "workspace start failed")
            }
        }
    }
    let messages = world
        .event_reader::<DomainAgentMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for message in messages {
        let Some((workspace, agent)) = agent_route(world, message.agent) else {
            continue;
        };
        broadcast_server_event(
            &connections,
            &ServerEvent::AgentMessage {
                message: AgentMessageDto {
                    id: message.id,
                    workspace,
                    agent,
                    message: message.message,
                },
            },
        );
    }
    let failures = world
        .event_reader::<AgentFailure>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for failure in failures {
        let Some((workspace, agent)) = agent_route(world, failure.agent) else {
            continue;
        };
        broadcast_server_event(
            &connections,
            &ServerEvent::AgentFailure {
                failure: AgentFailureDto {
                    id: failure.id,
                    workspace,
                    agent,
                    kind: format!("{:?}", failure.kind),
                    message: failure.message,
                },
            },
        );
    }
}

fn route_agent_message(
    world: &World,
    id: String,
    workspace: margatroid_protocol::WorkspaceRefDto,
    agent: Option<String>,
    content: String,
) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("message id cannot be empty".into());
    }
    if content.trim().is_empty() {
        return Err("message content cannot be empty".into());
    }
    let project_root = PathBuf::from(&workspace.project_root);
    let workspace_entity = world
        .workspace(&project_root, &workspace.name)
        .ok_or_else(|| "workspace was not found or is not ready".to_owned())?;
    let agent_entity = match agent {
        Some(name) if name.trim().is_empty() => return Err("agent name cannot be empty".into()),
        Some(name) => world
            .workspace_agent(workspace_entity, &name)
            .ok_or_else(|| format!("agent `{name}` was not found in the workspace"))?,
        None => world
            .workspace_manager(workspace_entity)
            .ok_or_else(|| "workspace manager is not ready".to_owned())?,
    };
    world.send_event(DomainAgentMessage {
        id,
        agent: agent_entity,
        message: Message::User { content },
        intent: MessageIntent::UserWithoutToolCalls,
    });
    Ok(())
}

fn workspace_info(world: &World, workspace: core_plugin::Entity) -> Option<WorkspaceInfoDto> {
    world
        .get_component::<WorkspaceConfiguration>(workspace)
        .map(|configuration| WorkspaceInfoDto::from_definition(configuration.definition()))
}

fn agent_route(
    world: &World,
    agent: core_plugin::Entity,
) -> Option<(margatroid_protocol::WorkspaceRefDto, String)> {
    let workspace = world.workspace_of(agent)?;
    let name = world
        .get_component::<WorkspaceAgents>(workspace)?
        .iter()
        .find_map(|(name, entity)| (entity == agent).then_some(name.to_owned()))?;
    let workspace = workspace_info(world, workspace)?.reference();
    Some((workspace, name))
}

fn broadcast_server_event(connections: &WebSocketConnections, event: &ServerEvent) {
    let Ok(encoded) = serde_json::to_string(event) else {
        return;
    };
    for sender in connections.unnamed() {
        let _ = sender.try_send(WebSocketMessage::Text(encoded.clone().into()));
    }
}

fn parse_args<I>(arguments: I) -> Result<DaemonConfig, String>
where
    I: IntoIterator<Item = String>,
{
    let mut arguments = arguments.into_iter();
    let mut bind = DEFAULT_BIND.parse().expect("default bind address is valid");
    let mut data_root = default_data_root();
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Err(usage().to_owned());
        }
        if argument == "--bind" {
            bind = parse_bind(arguments.next().ok_or("--bind requires an address")?)?;
            continue;
        }
        if let Some(value) = argument.strip_prefix("--bind=") {
            bind = parse_bind(value.to_owned())?;
            continue;
        }
        if argument == "--data-root" {
            data_root = PathBuf::from(arguments.next().ok_or("--data-root requires a directory")?);
            continue;
        }
        if let Some(value) = argument.strip_prefix("--data-root=") {
            if value.is_empty() {
                return Err("--data-root requires a directory".into());
            }
            data_root = PathBuf::from(value);
            continue;
        }
        return Err(format!("unknown option '{argument}'"));
    }
    Ok(DaemonConfig { bind, data_root })
}

fn parse_bind(value: String) -> Result<SocketAddr, String> {
    value
        .parse()
        .map_err(|error| format!("invalid bind address '{value}': {error}"))
}

fn default_data_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_DATA_ROOT)
}

fn absolute_path(path: &Path) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn usage() -> &'static str {
    "Usage: margatroid-daemon [--bind HOST:PORT] [--data-root DIRECTORY]\n\nStart the Margatroid backend WebSocket server."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_local_server_and_home_data_root() {
        let config = parse_args(std::iter::empty()).unwrap();
        assert_eq!(config.bind, DEFAULT_BIND.parse().unwrap());
        assert!(config.data_root.ends_with(DEFAULT_DATA_ROOT));
    }

    #[test]
    fn accepts_bind_and_data_root_options() {
        let config = parse_args([
            "--bind".into(),
            "0.0.0.0:4000".into(),
            "--data-root=/tmp/margatroid-test".into(),
        ])
        .unwrap();
        assert_eq!(config.bind, "0.0.0.0:4000".parse().unwrap());
        assert_eq!(config.data_root, PathBuf::from("/tmp/margatroid-test"));
    }
}
