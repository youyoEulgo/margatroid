use crate::handler::{handle_command, handle_command_result, handle_queue};
use crate::types::{CommandError, CommandOutput};
use core_plugin::World;
use mcl_core::{MclCommand, MclQueue};

pub(crate) fn mcl_command_system(world: &mut World) {
    let requests = world
        .event_reader::<MclCommand>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_command(world, request);
    }
    let results = world
        .event_reader::<Result<CommandOutput, CommandError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for result in results {
        handle_command_result(world, result);
    }
    let targets = world.query_with::<MclQueue>().result();
    for target in targets {
        handle_queue(world, target);
    }
}
