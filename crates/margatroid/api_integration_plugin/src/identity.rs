use agent_plugin::AgentIdentity;
use core_plugin::{Entity, World};
use margatroid_protocol::{WorkspaceInfoDto, WorkspaceReferenceDto};
use margatroid_types::AgentReference;
use workspace_plugin::{WorkspaceAgents, WorkspaceConfiguration, WorldWorkspaceExt};

pub(crate) fn workspace_info(world: &World, workspace: Entity) -> Option<WorkspaceInfoDto> {
    world
        .get_component::<WorkspaceConfiguration>(workspace)
        .map(|configuration| WorkspaceInfoDto::from_domain(configuration.definition()))
}

pub(crate) fn agent_route(
    world: &World,
    reference: &AgentReference,
) -> Option<(WorkspaceReferenceDto, String)> {
    let agent = match reference {
        AgentReference::Entity(agent) => *agent,
        AgentReference::Id(id) => world
            .query_with::<AgentIdentity>()
            .result()
            .into_iter()
            .find(|entity| {
                world
                    .get_component::<AgentIdentity>(*entity)
                    .is_some_and(|identity| identity.id() == id)
            })?,
    };
    let workspace = world.workspace_of(agent)?;
    let name = world
        .get_component::<WorkspaceAgents>(workspace)?
        .iter()
        .find_map(|(name, entity)| (entity == agent).then_some(name.to_owned()))?;
    let workspace = workspace_info(world, workspace)?.reference();
    Some((workspace, name))
}
