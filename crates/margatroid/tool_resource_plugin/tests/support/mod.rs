use core_plugin::{App, Entity};
use margatroid_types::agent_components::{Agent, AgentInfo, AgentState};
use margatroid_types::block_storage::AgentDatabaseHandle;
use margatroid_types::resource_components::AgentImageModelConfig;
use memory_plugin::SqliteAgentDatabase;
use resource_id_plugin::{ResourceID, WorldResourceExt};
use std::sync::Arc;

#[derive(Default)]
struct DatabaseFixtures(Vec<tempfile::TempDir>);
impl core_plugin::Resource for DatabaseFixtures {}
pub fn agent(app: &mut App, dependencies: &[&str]) -> (Entity, ResourceID) {
    app.world_mut().register_library::<Agent>("agent").unwrap();
    let entity = app.world_mut().spawn();
    let id = ResourceID::parse("agent:local/test").unwrap();
    let root = tempfile::tempdir().unwrap();
    let database = AgentDatabaseHandle::new(
        SqliteAgentDatabase::open(root.path().join("agent.sqlite")).unwrap(),
    );
    if app.world().get_resource::<DatabaseFixtures>().is_none() {
        app.world_mut().insert_resource(DatabaseFixtures::default());
    }
    app.world_mut()
        .get_resource_mut::<DatabaseFixtures>()
        .unwrap()
        .0
        .push(root);
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
        state: AgentState::Idle,
        history: database.history(),
        database,
        import_bindings: Default::default(),
        mcl: Default::default(),
        execution: Default::default(),
        turn: None,
    };
    app.world_mut().insert_library(entity, agent).unwrap();
    (entity, id)
}
