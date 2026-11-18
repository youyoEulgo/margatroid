//! Real per-agent storage for tool host integration fixtures.
pub fn mount(world: &mut core_plugin::World, agent: core_plugin::Entity, root: &std::path::Path) {
    let directory = root.join(format!(
        "agent-storage-{}-{}",
        agent.index(),
        agent.generation()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let storage = storage_core::open_storage(
        &directory.join("agent.sqlite"),
        &directory.join("settings.toml"),
        "[compaction]\nthreshold = 0.8\n[initialization]\ndefault=[]\ncustom=[]\n",
    )
    .unwrap();
    world.insert_component(agent, storage);
    if world.get_component::<address::Address>(agent).is_none() {
        world.insert_component(
            agent,
            address::Address::new(&format!(
                "agent:fixture/task-{}-{}",
                agent.index(),
                agent.generation()
            ))
            .unwrap(),
        );
    }
}

/// The tool-only fixture supplies the CONFIG domain through actual storage.
/// Full queue scheduling and nested calls are exercised in compose tests.
#[allow(dead_code)]
pub fn config_system(app: &mut core_plugin::App) {
    app.add_system(
        app_runtime_plugin::RuntimePlugin::UPDATE,
        |world: &mut core_plugin::World| {
            let requests = world
                .event_reader::<mcl_core::MclCommand>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            for request in requests {
                let operation = match mcl_core::parse_instruction(&request.instruction).unwrap() {
                    mcl_core::MclOperation::Config(mcl_core::ConfigOperation::Get) => {
                        storage_core::StorageOperation::ReadConfig
                    }
                    mcl_core::MclOperation::Config(mcl_core::ConfigOperation::Set {
                        path,
                        value,
                    }) => storage_core::StorageOperation::SetConfig { path, value },
                    _ => panic!("unexpected tool fixture MCL"),
                };
                let storage = world
                    .get_component::<storage_core::AgentStorage>(request.target)
                    .unwrap();
                let value = storage_core::execute_storage(storage, operation)
                    .map(|value| match value {
                        storage_core::StorageValue::Config(value) => {
                            mcl_core::MclValue::Config(value)
                        }
                        storage_core::StorageValue::Unit => mcl_core::MclValue::Unit,
                        _ => panic!("unexpected CONFIG response"),
                    })
                    .map_err(|err| {
                        mcl_core::MclError::new(
                            mcl_core::MclErrorKind::StorageFailed,
                            err.to_string(),
                        )
                    });
                request
                    .reply
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(value)
                    .ok();
            }
        },
    );
}
