use crate::{handler, types::*};
use client_core::ClientDisconnected;
use client_core::ClientRegistrationResult;
use core_plugin::World;
use workspace_core::{RestartWorkspaceResult, StartWorkspaceResult, StopWorkspaceResult};

pub(crate) fn dto_route_system(world: &mut World) {
    let disconnected = world
        .event_reader::<ClientDisconnected>()
        .into_iter()
        .map(|event| event.connection_id)
        .collect::<Vec<_>>();
    for connection in disconnected {
        handler::handle_disconnect(world, connection);
    }
    let requests = world
        .event_reader::<DtoRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handler::handle_dto_request(world, request);
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
    for failure in world
        .event_reader::<Result<ResponseReady, DtoAsyncError>>()
        .into_iter()
        .filter_map(|result| result.as_ref().err())
    {
        tracing::error!(reason=%failure.0, "DTO response task failed");
    }
    let snapshot_failed = world
        .event_reader::<Result<SnapshotReady, DtoAsyncError>>()
        .into_iter()
        .any(Result::is_err);
    if snapshot_failed {
        let cache = world.get_resource_mut::<BackendStateReportCache>().unwrap();
        cache.pending = None;
        cache.dirty = true;
        cache.retry_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
        tracing::warn!("DTO state snapshot task failed");
    }
    let responses = world
        .event_reader::<Result<ResponseReady, DtoAsyncError>>()
        .into_iter()
        .filter_map(|result| result.as_ref().ok().cloned())
        .collect::<Vec<_>>();
    for event in responses {
        handler::handle_response(world, event);
    }
    let snapshots = world
        .event_reader::<Result<SnapshotReady, DtoAsyncError>>()
        .into_iter()
        .filter_map(|result| result.as_ref().ok().cloned())
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
    for failure in world
        .event_reader::<Result<(), DtoAsyncError>>()
        .into_iter()
        .filter_map(|result| result.as_ref().err())
    {
        tracing::warn!(reason = %failure.0, "DTO log subscription failed");
    }
    if let Some(handle) = world.get_resource::<axum_plugin::AxumHandle>() {
        let address = handle.local_address();
        let stopped = handle.is_stopped();
        let result = handle.result();
        let status = world.get_resource_mut::<TransportStatus>().unwrap();
        if address != status.address {
            if let Some(address) = address {
                tracing::info!(%address, "WebSocket server started");
            }
            status.address = address;
        }
        if stopped && !status.stopped {
            match result {
                Some(Err(failure)) => tracing::error!(%failure,"WebSocket server failed"),
                _ => tracing::info!("WebSocket server stopped"),
            };
            status.stopped = true;
        }
    }
    handler::publish_state(world);
}
