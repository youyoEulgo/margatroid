use core_plugin::{App, Entity};
use margatroid_types::agent_components::{Agent, AgentInfo, AgentState};
use margatroid_types::history_components::{
    AgentHistoryHandle, AgentHistoryStore, HistoryError, HistoryRecord,
};
use margatroid_types::resource_components::AgentImageModelConfig;
use resource_id_plugin::{ResourceID, WorldResourceExt};
use std::sync::Arc;

struct History;
impl AgentHistoryStore for History {
    fn append_batch(&self, _: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        Ok(())
    }
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        Ok(Vec::new())
    }
}
pub fn agent(app: &mut App, dependencies: &[&str]) -> (Entity, ResourceID) {
    app.world_mut().register_library::<Agent>("agent").unwrap();
    let entity = app.world_mut().spawn();
    let id = ResourceID::parse("agent:local/test").unwrap();
    let agent = Agent {
        resourceid: id.clone(),
        info: AgentInfo {
            model: AgentImageModelConfig {
                model: "test".into(),
                temperature: None,
                max_output_tokens: None,
                top_p: None,
                stop: Vec::new(),
            },
            project_root: "/tmp/project".into(),
            home_root: "/tmp/home".into(),
            image: ResourceID::parse("image:local/test").unwrap(),
            image_dependencies: Arc::from(
                dependencies
                    .iter()
                    .map(|id| ResourceID::parse(id).unwrap())
                    .collect::<Vec<_>>(),
            ),
        },
        workspace: ResourceID::parse("workspace:local/test").unwrap(),
        sandbox: None,
        lua: Default::default(),
        state: AgentState::Running,
        history: AgentHistoryHandle::new(History),
        block: Default::default(),
        import_bindings: Default::default(),
        mcl: Default::default(),
        execution: Default::default(),
        turn: None,
    };
    app.world_mut().insert_library(entity, agent).unwrap();
    (entity, id)
}
