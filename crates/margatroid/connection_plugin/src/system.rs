use core_plugin::World;
use server_plugin::{RegisterConnection, WebSocketConnections};

use crate::handler::handle_register_connection;

pub(crate) fn connection_registration_system(world: &mut World) {
    let requests = world
        .event_reader::<RegisterConnection>()
        .into_iter()
        .collect::<Vec<_>>();
    let Some(connections) = world.get_resource::<WebSocketConnections>().cloned() else {
        return;
    };
    for request in requests {
        handle_register_connection(&connections, request);
    }
}
