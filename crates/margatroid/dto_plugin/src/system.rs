use core_plugin::World;
use server_plugin::WebSocketMessageReceived;

use crate::handler::{
    handle_collect_external_events, handle_inbound_message, handle_outbound_messages,
    handle_pending_mcl_responses,
};
use crate::WebSocketMessageSend;

pub(crate) fn dto_route_system(world: &mut World) {
    let received = world
        .event_reader::<WebSocketMessageReceived>()
        .into_iter()
        .map(|received| (received.connection_id, received.message.clone()))
        .collect::<Vec<_>>();
    for (connection_id, message) in received {
        handle_inbound_message(world, connection_id, message);
    }
    handle_pending_mcl_responses(world);
    let outgoing = world
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    handle_outbound_messages(world, outgoing);
}

pub(crate) fn collect_external_events_system(world: &mut World) {
    handle_collect_external_events(world);
}
