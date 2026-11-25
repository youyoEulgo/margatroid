use address::Address;
use agent_core::Agent;
use core_plugin::{Entity, World};
use margatroid_protocol::*;
use tool_core::ToolTasks;
use tool_resource_core::Tools;
use workspace_core::Workspace;

pub(crate) fn address(world: &World, entity: Entity) -> Option<String> {
    world
        .get_component::<Address>(entity)
        .map(|value| value.as_str().to_owned())
}

pub(crate) fn workspace_agent(record: workspace_core::WorkspaceAgent) -> WorkspaceAgentInfoDto {
    WorkspaceAgentInfoDto {
        id: record.id,
        parent_node: record.parent_node,
        node_name: record.node_name,
        alias: record.alias,
        category: format!("{:?}", record.category).to_lowercase(),
        tool_path: record.tool_path.to_string_lossy().into_owned(),
        tool_arguments: record.tool_arguments,
        previous_node_name: record.previous_node_name,
        previous_alias: record.previous_alias,
    }
}
pub(crate) fn workspace(world: &World, entity: Entity) -> Option<(WorkspaceInfoDto, u64)> {
    let value = world.get_component::<Workspace>(entity)?;
    let changes = value.database.subscribe_changes();
    let snapshot = changes.borrow();
    let snapshot = snapshot.as_ref()?;
    Some((
        WorkspaceInfoDto {
            address: address(world, entity)?,
            alias: snapshot.metadata.alias.clone(),
            project_root: value.project_root.to_string_lossy().into_owned(),
            status: format!("{:?}", value.state).to_lowercase(),
            agents: snapshot
                .agents
                .iter()
                .cloned()
                .map(workspace_agent)
                .collect(),
        },
        snapshot.revision,
    ))
}

pub(crate) fn workspaces(world: &World) -> Vec<WorkspaceInfoDto> {
    let mut rows: Vec<_> = world
        .query_with::<Workspace>()
        .result()
        .into_iter()
        .filter_map(|entity| workspace(world, entity).map(|(row, _)| row))
        .collect();
    rows.sort_by(|a, b| a.address.cmp(&b.address));
    rows
}

pub(crate) fn agents(world: &World) -> Vec<AgentStateDto> {
    let mut rows = world
        .query_with::<Agent>()
        .result()
        .into_iter()
        .filter_map(|entity| {
            let agent = world.get_component::<Agent>(entity)?;
            Some(AgentStateDto {
                address: address(world, entity)?,
                workspace: address(world, agent.workspace())?,
                creator: AgentCreatorDto {
                    address: address(world, agent.parent())?,
                    task_id: agent.task_id().into(),
                },
                status: format!("{:?}", agent.state()).to_lowercase(),
                tools: world
                    .get_component::<Tools>(entity)
                    .map(|tools| {
                        tools
                            .iter()
                            .map(|(alias, entry)| AgentToolDto {
                                alias: alias.clone(),
                                path: entry.path.to_string_lossy().into_owned(),
                                description: entry.description.clone(),
                                input_schema: entry.input_schema.clone(),
                                owner_task_id: entry.owner_task_id.clone(),
                                binding: entry.binding.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                sandboxes: world
                    .get_component::<sandbox_resource_core::Sandboxes>(entity)
                    .map(|sandboxes| {
                        sandboxes
                            .iter()
                            .map(|(alias, path)| AgentSandboxDto {
                                alias: alias.clone(),
                                path: path.to_string_lossy().into_owned(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                sandbox: world
                    .get_component::<sandbox_resource_core::SelectedSandbox>(entity)
                    .and_then(|sandbox| sandbox.alias.clone()),
                model: world
                    .get_component::<inference_core::AgentInferenceConfig>(entity)
                    .and_then(|config| {
                        let route = world
                            .get_resource::<inference_core::ModelRoutes>()?
                            .get(&config.parameters.model)?;
                        Some(AgentModelDto {
                            id: config.parameters.model.clone(),
                            model: route.model.clone(),
                            provider: route.provider.clone(),
                            api_type: route.api_type.clone(),
                            context_window_tokens: route.context_window_tokens,
                            thinking: route.thinking.clone(),
                            reasoning_effort: route.reasoning_effort.clone(),
                        })
                    }),
                error: world
                    .get_component::<ToolTasks>(agent.parent())
                    .and_then(|tasks| tasks.get(agent.task_id()).ok())
                    .and_then(|task| task.snapshot(agent.task_id()))
                    .and_then(|snapshot| snapshot.error)
                    .map(|e| ProtocolErrorDto::new(format!("{:?}", e.kind), e.message)),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.address.cmp(&b.address));
    rows
}

pub(crate) fn turn(world: &World, entity: Entity) -> Option<AgentTurnStateDto> {
    Some(AgentTurnStateDto {
        address: address(world, entity)?,
        turn: world.get_component::<Agent>(entity)?.turn().clone(),
    })
}

pub(crate) fn tasks(world: &World, entity: Entity) -> Option<AgentTaskStateDto> {
    Some(AgentTaskStateDto {
        address: address(world, entity)?,
        tasks: world
            .get_component::<ToolTasks>(entity)?
            .snapshot()
            .into_iter()
            .map(|task| AgentTaskDto {
                task_id: task.task_id,
                call_id: task.call_id,
                alias: task.alias,
                returned: task.returned,
                output: task.output,
                error: task.error.map(|failure| {
                    ProtocolErrorDto::new(format!("{:?}", failure.kind), &failure.message)
                }),
            })
            .collect(),
    })
}
