use crate::{Client, ClientError, ClientRegistrationResult, RegisteredClient, WorldClientExt};
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use resource_id_plugin::{ResourceID, WorldResourceExt};
use server_plugin::{
    RegisterConnection, WebSocketConnectionId, WebSocketConnections, WebSocketDisconnected,
    WebSocketNameError,
};

pub(crate) fn handle_client_registration(world: &mut World, request: &RegisterConnection) {
    let connections = world.get_resource::<WebSocketConnections>().cloned();
    let result = match connections {
        Some(connections) => register_client(world, &connections, request),
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

fn register_client(
    world: &mut World,
    connections: &WebSocketConnections,
    request: &RegisterConnection,
) -> Result<RegisteredClient, ClientError> {
    if request.id.trim().is_empty() {
        return Err(ClientError::InvalidRequest);
    }
    if !valid_client_type(&request.client_type) {
        return Err(ClientError::InvalidClientType);
    }
    if connections
        .get(request.connection_id)
        .is_none_or(|sender| sender.is_closed())
    {
        return Err(ClientError::ConnectionMissing);
    }
    if world.client_by_connection(request.connection_id).is_some() {
        return Err(ClientError::DuplicateClient);
    }
    let fallback = request.connection_id.get().to_string();
    let name = request.name.as_deref().unwrap_or(&fallback);
    let resource_id = ResourceID::new("client", &request.client_type, name, Some(fallback.clone()))
        .map_err(|_| ClientError::InvalidResourceID)?;
    let owners = world
        .find_resource_entities(&resource_id)
        .map_err(|_| ClientError::ResourceSetupFailed)?;
    if !owners.is_empty() {
        return Err(ClientError::DuplicateClient);
    }
    if world
        .query_with::<Client>()
        .result()
        .into_iter()
        .any(|entity| {
            world
                .get_component::<Client>(entity)
                .is_some_and(|client| client.name == name)
        })
    {
        return Err(ClientError::DuplicateName);
    }
    let old_name = connections.name(request.connection_id).unwrap_or_default();
    let old_type = connections
        .connection_type(request.connection_id)
        .unwrap_or_default();
    connections
        .set_name(request.connection_id, name)
        .map_err(|error| match error {
            WebSocketNameError::NameAlreadyExists { .. } => ClientError::DuplicateName,
            _ => ClientError::ConnectionMissing,
        })?;
    if !connections.set_connection_type(request.connection_id, &request.client_type) {
        let _ = connections.set_name(request.connection_id, old_name);
        return Err(ClientError::ConnectionMissing);
    }
    let entity = world.spawn();
    let client = Client {
        resource_id: resource_id.clone(),
        connection_id: request.connection_id,
        client_type: request.client_type.clone(),
        name: name.into(),
    };
    if world.insert_library(entity, client).is_err() {
        world.despawn(entity);
        let _ = connections.set_name(request.connection_id, old_name);
        connections.set_connection_type(request.connection_id, old_type);
        return Err(ClientError::ResourceSetupFailed);
    }
    if connections
        .get(request.connection_id)
        .is_none_or(|sender| sender.is_closed())
    {
        world.despawn(entity);
        let _ = connections.set_name(request.connection_id, old_name);
        connections.set_connection_type(request.connection_id, old_type);
        return Err(ClientError::ConnectionMissing);
    }
    Ok(RegisteredClient {
        client: entity,
        resource_id,
    })
}

pub fn registered_client(
    world: &World,
    connection_id: WebSocketConnectionId,
) -> Option<ResourceID> {
    world
        .client_by_connection(connection_id)
        .map(|(_, client)| client.resource_id.clone())
}
pub(crate) fn handle_client_disconnect(world: &mut World, event: &WebSocketDisconnected) {
    let connection_id = event.connection_id;
    let entities = world
        .query_with::<Client>()
        .result()
        .into_iter()
        .filter(|entity| {
            world
                .get_component::<Client>(*entity)
                .is_some_and(|client| client.connection_id == connection_id)
        })
        .collect::<Vec<_>>();
    for entity in entities {
        world.despawn(entity);
    }
}
fn valid_client_type(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}
