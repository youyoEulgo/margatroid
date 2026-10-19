use core_plugin::World;
use server_plugin::{RegisterConnection, WebSocketDisconnected};

use crate::handler::{handle_client_disconnect, handle_client_registration};

pub(crate) fn client_registration_system(world: &mut World) {
    let requests = world
        .event_reader::<RegisterConnection>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_client_registration(world, &request);
    }
}

pub(crate) fn client_disconnect_system(world: &mut World) {
    let events = world
        .event_reader::<WebSocketDisconnected>()
        .into_iter()
        .map(|event| WebSocketDisconnected {
            connection_id: event.connection_id,
            reason: event.reason.clone(),
        })
        .collect::<Vec<_>>();
    for event in events {
        handle_client_disconnect(world, &event);
    }
}
