use core_plugin::{App, Component, World};
use resource_id_plugin::{
    LibraryComponent, ResourceError, ResourceID, ResourceInner, ResourcePlugin,
    ResourcePluginInstalled, ResourceSet, ResourceTypeRegistry, ResourcesComponent,
    WorldResourceExt,
};

struct Agent(ResourceID);
impl Component for Agent {}
impl LibraryComponent for Agent {
    fn resource_id(&self) -> &ResourceID {
        &self.0
    }
}
struct Workspace(ResourceID);
impl Component for Workspace {}
impl LibraryComponent for Workspace {
    fn resource_id(&self) -> &ResourceID {
        &self.0
    }
}
struct Tool(&'static str);
impl ResourceInner for Tool {
    fn get_alias(&self) -> &str {
        self.0
    }
}
struct Image;

fn id(value: &str) -> ResourceID {
    ResourceID::parse(value).unwrap()
}
fn world() -> World {
    let mut world = World::new();
    world.insert_resource(ResourcePluginInstalled);
    world.insert_resource(ResourceTypeRegistry::default());
    world
}

#[test]
fn plugin_installs_registry_and_rejects_duplicate_installation() {
    let mut app = App::new();
    app.add_plugin(ResourcePlugin);
    assert!(app.world().contains_resource::<ResourceTypeRegistry>());
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.add_plugin(ResourcePlugin);
    }))
    .is_err());
}

#[test]
fn registration_is_bijective_idempotent_and_requires_installation() {
    assert_eq!(
        World::new().register_library::<Agent>("agent"),
        Err(ResourceError::PluginMissing)
    );
    let mut world = world();
    world.register_library::<Agent>("agent").unwrap();
    world.register_library::<Agent>("agent").unwrap();
    assert!(matches!(
        world.register_library::<Workspace>("agent"),
        Err(ResourceError::TypeRegistrationConflict { .. })
    ));
    assert!(matches!(
        world.register_library::<Agent>("other"),
        Err(ResourceError::TypeRegistrationConflict { .. })
    ));
    assert!(matches!(
        world.register_resources::<ResourceSet<Image>>("Image"),
        Err(ResourceError::InvalidResourceType { .. })
    ));
    assert!(matches!(
        id("tool:local/a").find_entities(&world),
        Err(ResourceError::TypeNotRegistered { .. })
    ));
    world
        .register_resources::<ResourceSet<Image>>("image")
        .unwrap();
    assert_eq!(id("image:local/a").find_entities(&world).unwrap(), vec![]);
}

#[test]
fn library_insertion_is_checked_atomic_and_queries_actual_world() {
    let mut world = world();
    world.register_library::<Agent>("agent").unwrap();
    world.register_library::<Workspace>("workspace").unwrap();
    let a = world.spawn();
    let b = world.spawn();
    let first = id("agent:local/a");
    let second = id("agent:local/b");
    world.insert_library(a, Agent(first.clone())).unwrap();
    world.insert_library(b, Agent(second.clone())).unwrap();
    assert!(matches!(
        world.insert_library(b, Agent(first.clone())),
        Err(ResourceError::LibraryAddressOccupied { .. })
    ));
    assert_eq!(
        world.get_component::<Agent>(b).unwrap().resource_id(),
        &second
    );
    assert!(matches!(
        world.insert_library(a, Workspace(id("workspace:local/a"))),
        Err(ResourceError::LibraryConflict { .. })
    ));
    assert!(matches!(
        world.insert_library(a, Agent(id("workspace:local/a"))),
        Err(ResourceError::AddressTypeMismatch { .. })
    ));
    world.insert_component(b, Agent(first.clone()));
    assert_eq!(first.find_entities(&world).unwrap(), vec![a, b]);
    world.despawn(a);
    assert_eq!(first.find_entities(&world).unwrap(), vec![b]);
    assert!(matches!(
        world.insert_library(a, Agent(first)),
        Err(ResourceError::EntityNotAlive { .. })
    ));
}

#[test]
fn sets_are_ordered_replace_locally_and_allow_alias_free_images() {
    let mut world = world();
    world
        .register_resources::<ResourceSet<Tool>>("tool")
        .unwrap();
    world
        .register_resources::<ResourceSet<Image>>("image")
        .unwrap();
    let a = world.spawn();
    let b = world.spawn();
    let first = id("tool:local/a");
    let second = id("tool:local/b");
    let mut set = ResourceSet::new();
    set.insert(second.clone(), Tool("b"));
    set.insert(first.clone(), Tool("a"));
    assert_eq!(
        set.list_id().cloned().collect::<Vec<_>>(),
        vec![first.clone(), second.clone()]
    );
    assert_eq!(set.get_by_id(&first).unwrap().get_alias(), "a");
    assert_eq!(
        set.insert(first.clone(), Tool("new")).unwrap().get_alias(),
        "a"
    );
    world.insert_resources(a, set).unwrap();
    let mut other = ResourceSet::new();
    other.insert(first.clone(), Tool("other"));
    world.insert_resources(b, other).unwrap();
    assert_eq!(world.find_resource_entities(&first).unwrap(), vec![a, b]);
    let mut invalid = ResourceSet::new();
    invalid.insert(id("skill:local/a"), Tool("invalid"));
    assert!(matches!(
        world.insert_resources(a, invalid),
        Err(ResourceError::AddressTypeMismatch { .. })
    ));
    assert_eq!(
        world.get_component::<ResourceSet<Tool>>(a).unwrap().len(),
        2
    );
    let image_id = id("image:local/a");
    let mut images = ResourceSet::new();
    images.insert(image_id.clone(), Image);
    assert!(images.get(&image_id).is_some());
    world.insert_resources(a, images).unwrap();
    assert_eq!(image_id.find_entities(&world).unwrap(), vec![a]);
    world
        .insert_resources(a, ResourceSet::<Tool>::default())
        .unwrap();
    assert_eq!(first.find_entities(&world).unwrap(), vec![b]);
}

struct DuplicateSet(Vec<ResourceID>);
impl Component for DuplicateSet {}
impl ResourcesComponent for DuplicateSet {
    type Inner = Tool;
    fn list_id(&self) -> impl Iterator<Item = &ResourceID> + '_ {
        self.0.iter()
    }
    fn get_by_id(&self, _: &ResourceID) -> Option<&Tool> {
        None
    }
    fn resource_ids(&self) -> impl Iterator<Item = &ResourceID> + '_ {
        self.0.iter()
    }
}

#[test]
fn custom_collections_are_validated_and_unregistered_components_rejected() {
    let mut world = world();
    let entity = world.spawn();
    assert!(matches!(
        world.insert_resources(entity, ResourceSet::<Image>::new()),
        Err(ResourceError::ComponentNotRegistered { .. })
    ));
    world.register_resources::<DuplicateSet>("tool").unwrap();
    let address = id("tool:local/a");
    assert!(matches!(
        world.insert_resources(entity, DuplicateSet(vec![address.clone(), address])),
        Err(ResourceError::DuplicateEntryID { .. })
    ));
    assert!(world.get_component::<DuplicateSet>(entity).is_none());
}

#[test]
fn values_validate_every_segment_and_use_latest_without_type_annotations() {
    assert_eq!(
        ResourceID::new("tool", "local", "a", None).unwrap().tag(),
        "latest"
    );
    for value in [
        "tool:local/a:",
        "tool:local/a:-bad",
        "tool:local/a:x:y",
        "tool:/a",
        "tool:local/..",
        "tool:local/a\\b",
        "tool:local/a\u{0085}",
    ] {
        assert!(ResourceID::parse(value).is_err(), "{value}");
    }
}
