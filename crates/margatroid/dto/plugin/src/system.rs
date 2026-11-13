use crate::{handler, types::*};
use core_plugin::World;

pub(crate) fn dto_route_system(world: &mut World) {
    let requests = world
        .event_reader::<DtoRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handler::handle_dto_request(world, request);
    }
}
pub(crate) fn abort_control_system(world: &mut World) {
    let requests = world
        .event_reader::<AbortTurnControl>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handler::handle_abort_control(world, request);
    }
}
