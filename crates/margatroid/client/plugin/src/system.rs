use client_core::{ClientDisconnected, RegisterClient};
use core_plugin::World;

use crate::handler::{handle_client_disconnect, handle_client_registration};

pub(crate) fn client_registration_system(world: &mut World) {
    let requests = world
        .event_reader::<RegisterClient>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_client_registration(world, &request);
    }
}

pub(crate) fn client_disconnect_system(world: &mut World) {
    let events = world
        .event_reader::<ClientDisconnected>()
        .into_iter()
        .map(|event| ClientDisconnected {
            connection_id: event.connection_id,
        })
        .collect::<Vec<_>>();
    for event in events {
        handle_client_disconnect(world, &event);
    }
}
