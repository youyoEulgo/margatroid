use address::Address;
use agent_core::{Agent, AgentState};
use core_plugin::{Entity, World};
use margatroid_protocol::*;
use std::collections::BTreeMap;
use tool_core::ToolTasks;
use tool_resource_core::Tools;
use workspace_core::{Workspace, WorkspaceDefinitions};

pub(crate) fn address(world: &World, entity: Entity) -> Option<String> {
    world
        .get_component::<Address>(entity)
        .map(|value| value.as_str().to_owned())
}

pub(crate) fn workspaces(world: &World) -> Vec<WorkspaceInfoDto> {
    let running: BTreeMap<_, _> = world
        .query_with::<Workspace>()
        .result()
        .into_iter()
        .filter_map(|entity| {
            world
                .get_component::<Workspace>(entity)
                .map(|item| (item.definition_path.clone(), entity))
        })
        .collect();
    let mut definitions: BTreeMap<_, _> = world
        .get_resource::<WorkspaceDefinitions>()
        .map(|catalog| catalog.list())
        .unwrap_or_default()
        .into_iter()
        .collect();
    for (path, entity) in &running {
        if let Some(definition) = world
            .get_component::<Workspace>(*entity)
            .and_then(|w| w.definition.clone())
        {
            definitions.entry(path.clone()).or_insert(definition);
        }
    }
    definitions
        .into_iter()
        .map(|(path, definition)| {
            let live = running.get(&path).copied();
            WorkspaceInfoDto {
                path: path.to_string_lossy().into_owned(),
                definition: WorkspaceDefinitionDto {
                    name: definition.name,
                    project_root: definition.project_root.to_string_lossy().into_owned(),
                    agents: definition
                        .members
                        .into_iter()
                        .map(|member| WorkspaceAgentDefinitionDto {
                            name: member.name,
                            driver: member.driver,
                            home_root: member.home_root.to_string_lossy().into_owned(),
                        })
                        .collect(),
                },
                status: live
                    .and_then(|entity| world.get_component::<Workspace>(entity))
                    .map(|item| format!("{:?}", item.state).to_lowercase())
                    .unwrap_or_else(|| "inactive".into()),
                address: live.and_then(|entity| address(world, entity)),
                error: live
                    .and_then(|entity| {
                        world.get_component::<workspace_core::WorkspaceFailure>(entity)
                    })
                    .map(|f| ProtocolErrorDto::new(format!("{:?}", f.0.kind), f.0.message.clone())),
            }
        })
        .collect()
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
                workspace: address(world, agent.workspace)?,

                status: match agent.state {
                    AgentState::Idle => "ready".into(),
                    _ => format!("{:?}", agent.state).to_lowercase(),
                },
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
                    .get_resource::<crate::types::SyncState>()
                    .and_then(|state| state.errors.get(&entity))
                    .cloned(),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.address.cmp(&b.address));
    rows
}

pub(crate) fn turn(world: &World, entity: Entity) -> Option<AgentTurnStateDto> {
    Some(AgentTurnStateDto {
        address: address(world, entity)?,
        turn: world.get_component::<Agent>(entity)?.turn.clone(),
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
