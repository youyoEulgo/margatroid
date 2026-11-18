use crate::handler::{handle_command, handle_queue};
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
    let targets = world.query_with::<MclQueue>().result();
    for target in targets {
        handle_queue(world, target);
    }
}
