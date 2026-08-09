use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_protocol::{
    AgentFailureDto, AgentMessageDto, BackendStateDto, IntoDto, ProtocolErrorKind, ServerMessage,
};
use margatroid_types::{AgentFailure, AgentMessage};
use server_plugin::{ServerFailed, ServerStarted, ServerStopped};
use workspace_plugin::StartWorkspaceResult;

use crate::{WebSocketMessageSend, WebSocketMessageTarget};

pub(crate) fn collect_external_events_system(world: &mut World, frontend_type: &str) {
    report_server_events(world);
    report_workspace_events(world);
    report_agent_messages(world);
    report_agent_failures(world);
    sync_frontend_state(world, frontend_type);
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
            Ok(workspace) => {
                tracing::info!(request_id = %result.id, "workspace started");
                match (*workspace).into_dto(world) {
                    Ok(info) => world.send_event(WebSocketMessageSend {
                        target: WebSocketMessageTarget::Broadcast,
                        message: ServerMessage::WorkspaceStarted {
                            id: result.id.clone(),
                            workspace: info,
                        },
                    }),
                    Err(error) => tracing::warn!(
                        request_id = %result.id,
                        error = %error,
                        "workspace DTO conversion failed"
                    ),
                }
            }
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
