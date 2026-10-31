use crate::{
    handler::{handle_storage_request, handle_storage_result},
    types::{StorageOutput, StorageTaskError},
};
use core_plugin::World;
use storage_core::StorageRequest;

pub(crate) fn storage_system(world: &mut World) {
    let requests: Vec<_> = world
        .event_reader::<StorageRequest>()
        .into_iter()
        .cloned()
        .collect();
    let results: Vec<_> = world
        .event_reader::<Result<StorageOutput, StorageTaskError>>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handle_storage_result(world, result);
    }
    for request in requests {
        handle_storage_request(world, request);
    }
}
