use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_protocol::{
    AgentFailureDto, AgentMessageDto, BackendStateDto, IntoDto, MessageDto, ProtocolErrorKind,
    ServerMessage, WorkspaceInfoDto,
};
use margatroid_types::{AgentFailure, AgentMessage};
use server_plugin::{
    ServerFailed, ServerStarted, ServerStopped, WebSocketConnected, WebSocketDisconnected,
    WebSocketProtocolFailed,
};
use workspace_plugin::{StartWorkspaceResult, StopWorkspaceByReferenceResult};

use crate::{WebSocketMessageSend, WebSocketMessageTarget};

pub(crate) fn collect_external_events_system(world: &mut World, frontend_type: &str) {
    report_server_events(world);
    report_workspace_events(world);
    report_workspace_stop_events(world);
    report_agent_messages(world);
    report_agent_failures(world);
    sync_frontend_state(world, frontend_type);
}

fn report_workspace_stop_events(world: &World) {
    for result in world.event_reader::<StopWorkspaceByReferenceResult>() {
        let message = match &result.result {
            Ok(()) => {
                tracing::info!(request_id = %result.id, workspace = %result.workspace.name, project_root = %result.workspace.project_root.display(), "workspace stopped");
                ServerMessage::WorkspaceStopped {
                    id: result.id.clone(),
                    workspace: result
                        .workspace
                        .clone()
                        .into_dto(())
                        .expect("workspace reference conversion cannot fail"),
                }
            }
            Err(error) => {
                tracing::error!(request_id = %result.id, workspace = %result.workspace.name, project_root = %result.workspace.project_root.display(), error = %error, "workspace stop failed");
                ServerMessage::WorkspaceStopFailed {
                    id: result.id.clone(),
                    error: error.to_string(),
                }
            }
        };
        world.send_event(WebSocketMessageSend {
            target: WebSocketMessageTarget::Broadcast,
            message,
        });
    }
}

fn report_server_events(world: &World) {
    for event in world.event_reader::<ServerStarted>() {
        tracing::info!(address = %event.address, "daemon WebSocket server started");
    }
    for event in world.event_reader::<ServerFailed>() {
        tracing::error!(error = %event.message, "daemon WebSocket server failed");
    }
    if !world.event_reader::<ServerStopped>().is_empty() {
        tracing::info!("daemon WebSocket server stopped");
    }
    for event in world.event_reader::<WebSocketConnected>() {
        tracing::info!(
            connection = event.connection_id.get(),
            "WebSocket connection opened"
        );
    }
    for event in world.event_reader::<WebSocketDisconnected>() {
        if let Some(reason) = &event.reason {
            tracing::info!(
                connection = event.connection_id.get(),
                code = reason.code(),
                reason = reason.reason(),
                "WebSocket connection closed"
            );
        } else {
            tracing::info!(
                connection = event.connection_id.get(),
                "WebSocket connection closed"
            );
        }
    }
    for event in world.event_reader::<WebSocketProtocolFailed>() {
        tracing::warn!(connection = event.connection_id.get(), error = %event.error, "WebSocket protocol failed");
    }
}

fn sync_frontend_state(world: &World, frontend_type: &str) {
    let state: BackendStateDto = match ().into_dto(world) {
        Ok(state) => state,
        Err(error) => {
            tracing::warn!(error = %error, "frontend state sync failed");
            return;
        }
    };
    world.send_event(WebSocketMessageSend {
        target: WebSocketMessageTarget::Type(frontend_type.into()),
        message: ServerMessage::StateSync { state },
    });
}

fn report_workspace_events(world: &World) {
    for result in world.event_reader::<StartWorkspaceResult>() {
        match &result.result {
            Ok(workspace) => match IntoDto::<WorkspaceInfoDto, _>::into_dto(*workspace, world) {
                Ok(info) => {
                    tracing::info!(request_id = %result.id, workspace = %info.name, project_root = %info.project_root, manager = %info.manager, agents = info.agents.len(), "workspace started");
                    world.send_event(WebSocketMessageSend {
                        target: WebSocketMessageTarget::Broadcast,
                        message: ServerMessage::WorkspaceStarted {
                            id: result.id.clone(),
                            workspace: info,
                        },
                    });
                }
                Err(error) => tracing::warn!(
                    request_id = %result.id,
                    error = %error,
                    "workspace DTO conversion failed"
                ),
            },
            Err(error) => {
                tracing::error!(request_id = %result.id, error = %error, "workspace start failed")
            }
        }
    }
}

fn report_agent_messages(world: &World) {
    let messages = world
        .event_reader::<AgentMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for message in messages {
        let message: AgentMessageDto = match (&message).into_dto(world) {
            Ok(message) => message,
            Err(error) if error.kind() == ProtocolErrorKind::UnsupportedMessage => continue,
            Err(error) => {
                tracing::warn!(request_id = %message.id, error = %error, "Agent message DTO conversion failed");
                continue;
            }
        };
        match &message.message {
            MessageDto::User { .. } => tracing::info!(
                request_id = %message.id,
                workspace = %message.workspace.name,
                agent = %message.agent,
                "user message routed"
            ),
            MessageDto::Assistant { tool_calls, .. } => tracing::info!(
                request_id = %message.id,
                workspace = %message.workspace.name,
                agent = %message.agent,
                tool_calls = tool_calls.len(),
                "assistant message produced"
            ),
        }
        world.send_event(WebSocketMessageSend {
            target: WebSocketMessageTarget::Broadcast,
            message: ServerMessage::AgentMessage { message },
        });
    }
}

fn report_agent_failures(world: &World) {
    let failures = world
        .event_reader::<AgentFailure>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for failure in failures {
        let dto: AgentFailureDto = match (&failure).into_dto(world) {
            Ok(dto) => dto,
            Err(error) => {
                tracing::warn!(request_id = %failure.id, error = %error, "Agent failure DTO conversion failed");
                continue;
            }
        };
        tracing::warn!(
            request_id = %failure.id,
            agent = %dto.agent,
            kind = %dto.kind,
            error = %failure.message,
            "agent turn failed"
        );
        world.send_event(WebSocketMessageSend {
            target: WebSocketMessageTarget::Broadcast,
            message: ServerMessage::AgentFailure { failure: dto },
        });
    }
}
