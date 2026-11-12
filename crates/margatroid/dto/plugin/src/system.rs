use crate::{handler, types::*};
use client_core::ClientDisconnected;
use core_plugin::World;

pub(crate) fn dto_route_system(world: &mut World) {
    let disconnected = world
        .event_reader::<ClientDisconnected>()
        .into_iter()
        .map(|v| v.connection_id)
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
pub(crate) fn dto_sync_system(world: &mut World) {
    let outgoing = world
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for message in outgoing {
        handler::handle_outbound(world, message);
    }
    let errors = world
        .event_reader::<agent_core::AgentException>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in errors {
        handler::handle_agent_exception(world, event);
    }
    for error in world
        .event_reader::<Result<(), DtoAsyncError>>()
        .into_iter()
        .filter_map(|v| v.as_ref().err())
    {
        tracing::warn!(reason=%error.0,"DTO synchronization failed");
    }
    handler::publish_state(world);
    crate::records::reconcile_record_watches(world);
    crate::ui::reconcile_ui_watches(world);
    crate::ui::publish_ui(world);
}
