use core_plugin::World;
use prompt_resource_core::ImportPrompt;

use crate::handler::{handle_import, handle_import_result};
use crate::types::{ImportError, ImportOutput};

pub(crate) fn prompt_resource_system(world: &mut World) {
    let requests = world
        .event_reader::<ImportPrompt>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let results = world
        .event_reader::<Result<ImportOutput, ImportError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_import(world, request);
    }
    for result in results {
        handle_import_result(world, result);
    }
}
