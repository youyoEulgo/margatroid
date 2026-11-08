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
        "[compaction]\nthreshold = 0.8\n",
    )
    .unwrap();
    world.insert_component(agent, storage);
}
