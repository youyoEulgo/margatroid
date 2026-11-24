#![allow(dead_code)]
use agent_core::{Agent, AgentState, AgentUpdateContext, WorldAgentExt};
use core_plugin::{Entity, World};
use std::{path::PathBuf, sync::Arc};
pub fn agent(
    _name: String,
    workspace: Entity,
    state: AgentState,
    turn: Option<String>,
    context: AgentUpdateContext,
) -> Agent {
    let mut world = World::new();
    world.insert_resource(context);
    let entity = world.spawn();
    world.insert_component(
        entity,
        Agent::new(workspace, workspace, "fixture-task".into(), PathBuf::new()),
    );
    if state != AgentState::Starting {
        world.agent_ready(entity).unwrap();
        if let Some(turn) = turn.filter(|t| !t.is_empty()) {
            world.agent_set_turn(entity, Some(turn)).unwrap();
        }
    }
    if state == AgentState::Closing {
        world.agent_begin_close(entity).unwrap();
    }
    world.remove_component::<Agent>(entity).unwrap()
}
pub fn workspace(
    alias: String,
    _definition: PathBuf,
    project_root: PathBuf,
    state: workspace_core::WorkspaceState,
) -> workspace_core::Workspace {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let dir = project_root.join(format!(
        "workspace-data-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    for kind in ["normal", "temporary", "archived"] {
        std::fs::create_dir_all(dir.join("agents").join(kind)).unwrap();
    }
    let database = workspace_core::WorkspaceDatabase::create(
        dir.join("workspace.sqlite"),
        workspace_core::WorkspaceMetadata {
            address: "/fixture".into(),
            alias: alias.clone(),
            project_root: project_root.clone(),
        },
    )
    .unwrap();
    let mut workspace = workspace_core::Workspace::new(alias, project_root, database);
    workspace.state = state;
    workspace
}
pub fn finish() -> tool_core::TaskFinish {
    Arc::new(|_| Box::pin(async { Ok(()) }))
}
