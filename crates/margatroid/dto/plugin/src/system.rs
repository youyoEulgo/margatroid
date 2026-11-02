use crate::{handler, types::*};
use client_core::ClientRegistrationResult;
use core_plugin::World;
use server_plugin::{
    ServerFailed, ServerStarted, ServerStopped, WebSocketConnected, WebSocketDisconnected,
    WebSocketMessageReceived, WebSocketProtocolFailed,
};
use workspace_core::{RestartWorkspaceResult, StartWorkspaceResult, StopWorkspaceResult};

pub(crate) fn dto_route_system(world: &mut World) {
    let disconnected = world
        .event_reader::<WebSocketDisconnected>()
        .into_iter()
        .map(|event| event.connection_id)
        .collect::<Vec<_>>();
    for connection in disconnected {
        handler::handle_disconnect(world, connection);
    }
    let messages = world
        .event_reader::<WebSocketMessageReceived>()
        .into_iter()
        .map(|event| (event.connection_id, event.message.clone()))
        .collect::<Vec<_>>();
    for (connection, message) in messages {
        handler::handle_inbound_message(world, connection, message);
    }
    let outgoing = world
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for message in outgoing {
        handler::handle_outbound(world, message);
    }
}

pub(crate) fn abort_control_system(world: &mut World) {
    let requests = world
        .event_reader::<AbortTurnControl>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handler::handle_abort_control(world, request);
    }
}

pub(crate) fn collect_external_events_system(world: &mut World) {
    let registrations = world
        .event_reader::<ClientRegistrationResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in registrations {
        handler::handle_registration(world, event);
    }
    let started = world
        .event_reader::<StartWorkspaceResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in started {
        handler::handle_workspace_result(world, event.id, event.result, "start");
    }
    let stopped = world
        .event_reader::<StopWorkspaceResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in stopped {
        handler::handle_workspace_result(world, event.id, event.result, "stop");
    }
    let restarted = world
        .event_reader::<RestartWorkspaceResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in restarted {
        handler::handle_workspace_result(world, event.id, event.result, "restart");
    }
    let responses = world
        .event_reader::<ResponseReady>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in responses {
        handler::handle_response(world, event);
    }
    let snapshots = world
        .event_reader::<SnapshotReady>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in snapshots {
        handler::handle_snapshot(world, event);
    }
    let logs = world
        .event_reader::<LogReady>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in logs {
        handler::handle_log(world, event);
    }
    let errors = world
        .event_reader::<agent_core::AgentException>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in errors {
        handler::handle_agent_exception(world, event);
    }
    let mut transport = world
        .event_reader::<ServerStarted>()
        .into_iter()
        .map(|event| TransportEvent::Started(event.address))
        .collect::<Vec<_>>();
    transport.extend(
        world
            .event_reader::<ServerFailed>()
            .into_iter()
            .map(|event| TransportEvent::Failed(event.message.clone())),
    );
    transport.extend(
        world
            .event_reader::<ServerStopped>()
            .into_iter()
            .map(|_| TransportEvent::Stopped),
    );
    transport.extend(
        world
            .event_reader::<WebSocketConnected>()
            .into_iter()
            .map(|event| TransportEvent::Connected(event.connection_id)),
    );
    transport.extend(
        world
            .event_reader::<WebSocketProtocolFailed>()
            .into_iter()
            .map(|event| TransportEvent::ProtocolFailed(event.connection_id)),
    );
    for event in transport {
        handler::handle_transport(world, event);
    }
    handler::publish_state(world);
}
