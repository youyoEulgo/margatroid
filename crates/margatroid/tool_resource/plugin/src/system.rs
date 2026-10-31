use core_plugin::World;
use tool_resource_core::ImportTool;

use crate::handler::{handle_import, handle_import_result};
use crate::types::{ImportError, ImportOutput};

pub(crate) fn tool_resource_system(world: &mut World) {
    let requests = world
        .event_reader::<ImportTool>()
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
