use api_plugin::{WebSocketMessageSend, WebSocketMessageTarget};
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_protocol::{AgentFailureDto, AgentMessageDto, ServerEvent};
use margatroid_types::{AgentFailure, AgentMessage, AgentReference};
use server_plugin::{ServerFailed, ServerStarted, ServerStopped};
use workspace_plugin::StartWorkspaceResult;

use crate::identity::{agent_route, workspace_info};

pub(crate) fn report_runtime_events(world: &mut World) {
    report_server_events(world);
    report_workspace_events(world);
    report_agent_messages(world);
    report_agent_failures(world);
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

fn report_workspace_events(world: &World) {
    for result in world.event_reader::<StartWorkspaceResult>() {
        match &result.result {
            Ok(workspace) => {
                tracing::info!(request_id = %result.id, "workspace started");
                if let Some(info) = workspace_info(world, *workspace) {
                    world.send_event(WebSocketMessageSend {
                        target: WebSocketMessageTarget::Broadcast,
                        message: ServerEvent::WorkspaceStarted {
                            id: result.id.clone(),
                            workspace: info,
                        },
                    });
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
        let Some((workspace, agent)) = agent_route(world, &message.agent) else {
            continue;
        };
        world.send_event(WebSocketMessageSend {
            target: WebSocketMessageTarget::Broadcast,
            message: ServerEvent::AgentMessage {
                message: AgentMessageDto {
                    id: message.id,
                    workspace,
                    agent,
                    message: message.message,
                },
            },
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
        let Some((workspace, agent)) = agent_route(world, &AgentReference::Entity(failure.agent))
        else {
            continue;
        };
        let kind = format!("{:?}", failure.kind);
        tracing::warn!(
            request_id = %failure.id,
            agent = %agent,
            kind = %kind,
            error = %failure.message,
            "agent turn failed"
        );
        world.send_event(WebSocketMessageSend {
            target: WebSocketMessageTarget::Broadcast,
            message: ServerEvent::AgentFailure {
                failure: AgentFailureDto {
                    id: failure.id,
                    workspace,
                    agent,
                    kind,
                    message: failure.message,
                },
            },
        });
    }
}
