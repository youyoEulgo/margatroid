use crate::handler;
use core_plugin::World;
use workspace_core::*;
pub(crate) fn load_workspace_system(w: &mut World) {
    handler::load_workspaces(w);
}
pub(crate) fn create_workspace_system(w: &mut World) {
    let req: Vec<_> = w
        .event_reader::<CreateWorkspace>()
        .into_iter()
        .cloned()
        .collect();
    for r in req {
        handler::create_workspace(w, r);
    }
}
pub(crate) fn start_workspace_system(w: &mut World) {
    let req: Vec<_> = w
        .event_reader::<StartWorkspace>()
        .into_iter()
        .cloned()
        .collect();
    for r in req {
        handler::start_workspace(w, r);
    }
    let completed: Vec<_> = w
        .event_reader::<crate::types::WorkspaceWorkCompleted>()
        .into_iter()
        .cloned()
        .collect();
    for completion in completed {
        completion
            .token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
}
pub(crate) fn delete_workspace_system(w: &mut World) {
    let results: Vec<_> = w
        .event_reader::<Result<crate::types::WorkspaceCleanupResult, WorkspaceError>>()
        .into_iter()
        .cloned()
        .collect();
    for r in results.into_iter().flatten() {
        handler::delete_result(w, r);
    }
    let req: Vec<_> = w
        .event_reader::<DeleteWorkspace>()
        .into_iter()
        .cloned()
        .collect();
    for r in req {
        handler::delete_workspace(w, r);
    }
}
pub(crate) fn start_agent_system(w: &mut World) {
    let req: Vec<_> = w
        .event_reader::<StartWorkspaceAgent<tool_core::ToolExecuteResponse, tool_core::ToolError>>()
        .into_iter()
        .cloned()
        .collect();
    for r in req {
        handler::start_agent(w, r);
    }
}
pub(crate) fn archive_agent_system(w: &mut World) {
    let req: Vec<_> = w
        .event_reader::<ArchiveAgent>()
        .into_iter()
        .cloned()
        .collect();
    for r in req {
        handler::archive_agent(w, r);
    }
}
