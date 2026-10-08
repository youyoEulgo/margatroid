use std::sync::Arc;

use core_plugin::App;
use resource_id_plugin::{ResourceIdPlugin, ResourceRef};
use resource_library_plugin::{
    install_resource, release_resource_owner, PromptResource, ResourceDefinition,
    ResourceLibraryError, ResourceOwners,
};

fn prompt(content: &str) -> ResourceDefinition {
    ResourceDefinition::Prompt(PromptResource {
        role: "system".into(),
        content: Arc::from(content),
    })
}

#[test]
fn imports_share_one_entity_and_one_owner_entry_per_agent() {
    let mut app = App::new();
    app.add_plugin(ResourceIdPlugin);
    let world = app.world_mut();
    let first = world.spawn();
    let second = world.spawn();
    let reference = ResourceRef::parse("prompt:local/shared").unwrap();
    let resource = install_resource(world, reference.clone(), first, prompt("old")).unwrap();
    assert_eq!(
        install_resource(world, reference.clone(), first, prompt("new")).unwrap(),
        resource
    );
    assert_eq!(
        world
            .get_component::<ResourceOwners>(resource)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        install_resource(world, reference, second, prompt("latest")).unwrap(),
        resource
    );
    assert_eq!(
        world
            .get_component::<ResourceOwners>(resource)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        &*world
            .get_component::<PromptResource>(resource)
            .unwrap()
            .content,
        "latest"
    );
    release_resource_owner(world, first);
    assert!(world.is_alive(resource));
    release_resource_owner(world, second);
    assert!(!world.is_alive(resource));
}

#[test]
fn mismatched_definition_does_not_create_a_resource() {
    let mut app = App::new();
    app.add_plugin(ResourceIdPlugin);
    let world = app.world_mut();
    let owner = world.spawn();
    let result = install_resource(
        world,
        ResourceRef::parse("tool:local/test").unwrap(),
        owner,
        prompt("text"),
    );
    assert!(matches!(result, Err(ResourceLibraryError::TypeMismatch)));
    assert!(world.query_with::<ResourceOwners>().result().is_empty());
}

#[test]
fn reused_entity_index_does_not_inherit_ownership() {
    let mut app = App::new();
    app.add_plugin(ResourceIdPlugin);
    let world = app.world_mut();
    let old = world.spawn();
    let resource = install_resource(
        world,
        ResourceRef::parse("prompt:local/shared").unwrap(),
        old,
        prompt("text"),
    )
    .unwrap();
    world.despawn(old);
    let new = world.spawn();
    assert_eq!(old.index(), new.index());
    assert_ne!(old.generation(), new.generation());
    release_resource_owner(world, new);
    assert!(world.is_alive(resource));
    release_resource_owner(world, old);
    assert!(!world.is_alive(resource));
}
