use core_plugin::{Entity, World};
use resource_id_plugin::{ResourceId, ResourceIdLookupError, WorldResourceIdExt};
use server_plugin::{
    RegisterConnection, WebSocketConnectionId, WebSocketConnections, WebSocketNameError,
};

use crate::{client_source, Client, ClientError};

pub(crate) fn handle_client_registration(
    world: &mut World,
    connections: &WebSocketConnections,
    request: &RegisterConnection,
) -> Result<Entity, ClientError> {
    let client_type = request.client_type.trim();
    if !valid_client_type(client_type) {
        return Err(ClientError::InvalidClientType);
    }
    let resource_id =
        client_resource_id(client_type, request.name.as_deref(), request.connection_id)?;
    if matches!(
        world.entity_by_resource_id(&resource_id),
        Ok(_) | Err(ResourceIdLookupError::Duplicate { .. })
    ) {
        return Err(ClientError::DuplicateClient);
    }
    let name = resource_id.name().to_owned();
    connections
        .set_name(request.connection_id, name.clone())
        .map_err(|error| match error {
            WebSocketNameError::NameAlreadyExists { .. } => ClientError::DuplicateName,
            _ => ClientError::ConnectionMissing,
        })?;
    if !connections.set_connection_type(request.connection_id, client_type) {
        return Err(ClientError::ConnectionMissing);
    }
    let entity = world.spawn();
    world.insert_component(entity, resource_id.clone());
    world.insert_component(
        entity,
        Client::new(request.connection_id, client_type.to_owned(), name.clone()),
    );
    tracing::info!(
        request_id = %request.id,
        connection = request.connection_id.get(),
        client_type,
        name,
        resource_id = %resource_id,
        "client registered"
    );
    Ok(entity)
}

pub(crate) fn handle_client_disconnect(world: &mut World, connection_id: WebSocketConnectionId) {
    let entity = world
        .query_with::<Client>()
        .result()
        .into_iter()
        .find(|entity| {
            world
                .get_component::<Client>(*entity)
                .is_some_and(|client| client.connection_id() == connection_id)
        });
    if let Some(entity) = entity {
        world.despawn(entity);
    }
}

fn client_resource_id(
    client_type: &str,
    name: Option<&str>,
    connection_id: WebSocketConnectionId,
) -> Result<ResourceId, ClientError> {
    let id = connection_id.get().to_string();
    let name = name.unwrap_or(&id);
    ResourceId::parse(client_source(Some(client_type), Some(name), connection_id))
        .map_err(|_| ClientError::InvalidResourceId)
}

fn valid_client_type(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}
