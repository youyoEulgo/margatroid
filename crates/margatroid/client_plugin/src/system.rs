use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use server_plugin::{RegisterConnection, WebSocketConnections, WebSocketDisconnected};

use crate::events::ClientRegistrationResult;
use crate::handler::{handle_client_disconnect, handle_client_registration};

pub(crate) fn client_registration_system(world: &mut World) {
    let requests = world
        .event_reader::<RegisterConnection>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let Some(connections) = world.get_resource::<WebSocketConnections>().cloned() else {
        return;
    };
    for request in requests {
        let result = handle_client_registration(world, &connections, &request);
        if let Err(error) = &result {
            tracing::warn!(
                request_id = %request.id,
                connection = request.connection_id.get(),
                error = %error,
                "client registration failed"
            );
        }
        world.send_event(ClientRegistrationResult {
            id: request.id.clone(),
            connection_id: request.connection_id,
            result,
        });
    }
}

pub(crate) fn client_disconnect_system(world: &mut World) {
    let connection_ids = world
        .event_reader::<WebSocketDisconnected>()
        .into_iter()
        .map(|event| event.connection_id)
        .collect::<Vec<_>>();
    for connection_id in connection_ids {
        handle_client_disconnect(world, connection_id);
    }
}
