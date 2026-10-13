use crate::handler;
use crate::{StartWorkspace, StopWorkspace, WorkspaceException, WorkspaceExecutionCloseOutput};
use core_plugin::World;
use margatroid_types::agent_events::{AgentExceptionResult, StartAgentResult, StopAgentResult};
use margatroid_types::agent_image_events::LoadAgentImageResult;

pub(crate) fn start_workspace_system(world: &mut World) {
    let events: Vec<_> = world
        .event_reader::<StartWorkspace>()
        .into_iter()
        .cloned()
        .collect();
    for event in events {
        handler::handle_start_workspace(world, event);
    }
    let events: Vec<_> = world
        .event_reader::<LoadAgentImageResult>()
        .into_iter()
        .cloned()
        .collect();
    for event in events {
        handler::handle_agent_image_result(world, event);
    }
    let events: Vec<_> = world
        .event_reader::<StartAgentResult>()
        .into_iter()
        .cloned()
        .collect();
    for event in events {
        handler::handle_start_agent_result(world, event);
    }
}
pub(crate) fn stop_workspace_system(world: &mut World) {
    let events: Vec<_> = world
        .event_reader::<StopWorkspace>()
        .into_iter()
        .cloned()
        .collect();
    for event in events {
        handler::handle_stop_workspace(world, event);
    }
    let events: Vec<_> = world
        .event_reader::<StopAgentResult>()
        .into_iter()
        .cloned()
        .collect();
    for event in events {
        handler::handle_stop_agent_result(world, event);
    }
    let events: Vec<_> = world
        .event_reader::<WorkspaceExecutionCloseOutput>()
        .into_iter()
        .cloned()
        .collect();
    for event in events {
        handler::handle_workspace_execution_closed(world, event);
    }
}
pub(crate) fn workspace_exception_system(world: &mut World) {
    let events: Vec<_> = world
        .event_reader::<AgentExceptionResult>()
        .into_iter()
        .cloned()
        .collect();
    for event in events {
        handler::handle_agent_exception_result(world, event);
    }
    let events: Vec<_> = world
        .event_reader::<WorkspaceException>()
        .into_iter()
        .cloned()
        .collect();
    for event in events {
        handler::handle_workspace_exception(world, event);
    }
}
