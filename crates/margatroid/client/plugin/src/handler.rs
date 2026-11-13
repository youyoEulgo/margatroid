use crate::{Client, ClientError, RegisteredClient, WorldClientExt};
use address::{Address, WorldAddressExt};
use client_core::{ClientDisconnected, ClientIdentity, Clients, RegisterClient};
use core_plugin::World;
use std::sync::Arc;

pub(crate) fn handle_client_registration(world: &mut World, request: &RegisterClient) {
    let Some(sender) = request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    else {
        return;
    };
    if sender.is_closed() {
        return;
    }
    let result = register_client(world, request);
    if result.is_ok() {
        world
            .get_resource::<Clients>()
            .unwrap()
            .initialize(request.connection_id);
    }
    if let Err(error) = &result {
        tracing::warn!(
            request_id = %request.id,
            connection = request.connection_id.get(),
            error = %error,
            "client registration failed"
        );
    }
    if let Err(Ok(registered)) = sender.send(result) {
        if world
            .client_by_connection(request.connection_id)
            .is_some_and(|(entity, _)| entity == registered.client)
        {
            world
                .get_resource::<Clients>()
                .unwrap()
                .remove(request.connection_id);
            world.despawn(registered.client);
        }
    }
}

fn register_client(
    world: &mut World,
    request: &RegisterClient,
) -> Result<RegisteredClient, ClientError> {
    if request.id.trim().is_empty() {
        return Err(ClientError::InvalidRequest);
    }
    if !valid_client_type(&request.client_type) {
        return Err(ClientError::InvalidClientType);
    }
    let clients = world
        .get_resource::<Clients>()
        .expect("Clients resource missing")
        .clone();
    if clients
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
    if !valid_client_name(name) {
        return Err(ClientError::InvalidName);
    }
    let address = Address::new(&format!("client:{}/{name}:{fallback}", request.client_type))
        .map_err(|_| ClientError::AddressConflict)?;
    if world
        .address_exists(address.as_str())
        .map_err(|_| ClientError::AddressConflict)?
    {
        return Err(ClientError::AddressConflict);
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
    let entity = world.spawn();
    let public_address = address.as_str().to_owned();
    let identity = Arc::new(ClientIdentity {
        client: entity,
        address: public_address.clone(),
        client_type: request.client_type.clone(),
        name: name.into(),
    });
    world.insert_component(
        entity,
        Client {
            connection_id: request.connection_id,
            identity: identity.clone(),
        },
    );
    world.insert_component(entity, address);
    if let Err(error) = clients.commit_registration(request.connection_id, identity) {
        world.despawn(entity);
        return Err(error);
    }
    Ok(RegisteredClient {
        client: entity,
        address: public_address,
        client_type: request.client_type.clone(),
    })
}

pub(crate) fn handle_client_disconnect(world: &mut World, event: &ClientDisconnected) {
    let connection_id = event.connection_id;
    world
        .get_resource::<Clients>()
        .unwrap()
        .remove(connection_id);
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

fn valid_client_name(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
}
