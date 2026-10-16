use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity};
use margatroid_types::agent_components::{Agent, AgentInfo, AgentState};
use margatroid_types::history_components::{
    AgentHistoryHandle, AgentHistoryStore, HistoryError, HistoryRecord,
};
use margatroid_types::lua_environment::*;
use margatroid_types::lua_runtime_components::DriverLuaVmInstance;
use margatroid_types::mcl_environment::MclEnvironmentFactory;
use margatroid_types::resource_components::AgentDriverModelConfig;
use mcl_plugin::MclPlugin;
use resource_id_plugin::{ResourcePlugin, WorldResourceExt};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Store(Mutex<Vec<HistoryRecord>>);
impl AgentHistoryStore for Store {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        self.0.lock().unwrap().extend(records);
        Ok(())
    }
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        Ok(self.0.lock().unwrap().clone())
    }
}
fn app() -> (App, Entity) {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(MclPlugin::default());
    app.world_mut().register_library::<Agent>("agent").unwrap();
    let entity = app.world_mut().spawn();
    app.world_mut()
        .insert_library(
            entity,
            Agent {
                resourceid: "agent:test/a".parse().unwrap(),
                info: AgentInfo {
                    model: AgentDriverModelConfig {
                        model: "test".into(),
                        temperature: None,
                        max_output_tokens: None,
                        top_p: None,
                        stop: vec![],
                    },
                    project_root: Default::default(),
                    home_root: Default::default(),
                    driver: "driver:test/a".parse().unwrap(),
                    driver_dependencies: Arc::from([]),
                },
                workspace: "workspace:test/a".parse().unwrap(),
                sandbox: None,
                lua: Default::default(),
                state: AgentState::Idle,
                history: AgentHistoryHandle::new(Store::default()),
                database: margatroid_types::block_storage::AgentDatabaseHandle::new(
                    memory_plugin::SqliteAgentDatabase::open(":memory:").unwrap(),
                ),
                import_bindings: Default::default(),
                mcl: Default::default(),
                execution: Default::default(),
                turn: None,
            },
        )
        .unwrap();
    app.world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .lua
        .instance = Some(DriverLuaVmInstance {
        owner: entity,
        start_request_id: "start".into(),
    });
    (app, entity)
}
fn call(
    app: &mut App,
    entity: Entity,
    text: &str,
    args: Option<LuaValue>,
) -> Result<LuaValue, margatroid_types::lua_runtime_components::LuaRuntimeError> {
    let env = app
        .world()
        .get_resource::<MclEnvironmentFactory>()
        .unwrap()
        .build();
    let LuaBindingValue::Function(host) = &env.globals[0].binding else {
        panic!()
    };
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    let mut values = vec![
        LuaValue::String("agent:test/a".into()),
        LuaValue::String(text.into()),
    ];
    if let Some(args) = args {
        values.push(args);
    }
    let mut future = host.call(
        LuaValue::Array(values),
        LuaEnvironmentContext {
            instance: agent.lua.instance.clone().unwrap(),
            values: Default::default(),
            wake: agent.lua.wake.clone(),
            execution: agent.execution.clone(),
            mcl: agent.mcl.drained.clone(),
        },
        Default::default(),
    );
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if let std::task::Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return result;
        }
        app.tick();
    }
    panic!("host did not reply")
}
#[test]
fn lua_empty_tables_are_arrays_only_in_declared_array_context() {
    let (mut app, entity) = app();
    let empty = LuaValue::Object(Default::default());
    assert!(call(
        &mut app,
        entity,
        "CREATE BLOCK b (m MESSAGE)",
        Some(empty.clone())
    )
    .is_ok());
    assert!(call(
        &mut app,
        entity,
        "INJECT ? TO b.m",
        Some(LuaValue::Array(vec![empty.clone()]))
    )
    .is_ok());
    assert!(call(
        &mut app,
        entity,
        "EMIT EFFECT sandbox_use ?",
        Some(LuaValue::Array(vec![empty.clone()]))
    )
    .is_ok());
    assert!(call(
        &mut app,
        entity,
        "IMPORT TOOL tool:test/unknown AS alias",
        None
    )
    .is_err());
    assert!(call(
        &mut app,
        entity,
        "EMIT EFFECT history_append ?",
        Some(LuaValue::Array(vec![LuaValue::Array(vec![empty.clone()])]))
    )
    .is_err());
    let mut object = std::collections::BTreeMap::new();
    object.insert("type".into(), LuaValue::String("user".into()));
    assert!(call(
        &mut app,
        entity,
        "INJECT ? TO b.m",
        Some(LuaValue::Array(vec![LuaValue::Object(object)]))
    )
    .is_err());
    assert!(mcl_plugin::parser::parse("INJECT ? TO b.m", &[serde_json::json!({})]).is_err());
}
