use core_plugin::World;
use sandbox_resource_core::ImportSandbox;

use crate::handler::{handle_import, handle_import_result};
use crate::types::{ImportOutput, ImportTaskError};

pub(crate) fn sandbox_resource_system(world: &mut World) {
    let requests = world
        .event_reader::<ImportSandbox>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let results = world
        .event_reader::<Result<ImportOutput, ImportTaskError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_import(world, request);
    }
    for result in results {
        match result {
            Ok(output) => handle_import_result(world, Ok(output)),
            Err(ImportTaskError::Validation(error)) => handle_import_result(world, Err(error)),
            Err(ImportTaskError::Runtime) => {} // The owned guard completed the original reply.
        }
    }
}
