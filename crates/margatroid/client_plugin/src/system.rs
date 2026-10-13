use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use server_plugin::{RegisterConnection, WebSocketConnections, WebSocketDisconnected};

use crate::handler::{handle_client_disconnect, handle_client_registration};
use crate::{ClientError, ClientRegistrationResult};

pub(crate) fn client_registration_system(world: &mut World) {
    let requests = world
        .event_reader::<RegisterConnection>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let connections = world.get_resource::<WebSocketConnections>().cloned();
    for request in requests {
        let result = match &connections {
            Some(connections) => handle_client_registration(world, connections, &request),
            None => Err(ClientError::ConnectionMissing),
        };
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
