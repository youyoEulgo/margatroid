use address::{Address, AddressError, WorldAddressExt};
use core_plugin::{Component, World};

struct Domain;
impl Component for Domain {}

#[test]
fn queries_validate_input_and_find_only_address_components() {
    let mut world = World::new();
    let unrelated = world.spawn();
    world.insert_component(unrelated, Domain);
    assert!(!world.address_exists("tool:margatroid/search:dev").unwrap());
    assert!(matches!(
        world.entity_by_address("tool:margatroid/search:dev"),
        Err(AddressError::EntityMissing)
    ));
    assert!(matches!(
        world.address_exists("bad"),
        Err(AddressError::InvalidFormat)
    ));
    assert!(matches!(
        world.entity_by_address("bad"),
        Err(AddressError::InvalidFormat)
    ));
    let target = world.spawn();
    world.insert_component(target, Address::new("tool:margatroid/search:dev").unwrap());
    world.insert_component(target, Domain);
    assert!(world.address_exists("tool:margatroid/search:dev").unwrap());
    assert_eq!(
        world
            .entity_by_address("tool:margatroid/search:dev")
            .unwrap(),
        target
    );
    assert_eq!(
        world
            .entity_by_address("tool:margatroid/search:dev")
            .unwrap(),
        target
    );
}

#[test]
fn duplicate_entities_fail_unique_lookup_and_remain_occupied() {
    let mut world = World::new();
    let first = world.spawn();
    let second = world.spawn();
    world.insert_component(first, Address::new("agent:margatroid/example:dev").unwrap());
    world.insert_component(
        second,
        Address::new("agent:margatroid/example:dev").unwrap(),
    );
    assert!(matches!(
        world.entity_by_address("agent:margatroid/example:dev"),
        Err(AddressError::DuplicateAddress)
    ));
    assert!(world
        .address_exists("agent:margatroid/example:dev")
        .unwrap());
    world.remove_component::<Address>(first);
    assert_eq!(
        world
            .entity_by_address("agent:margatroid/example:dev")
            .unwrap(),
        second
    );
    world.insert_component(
        second,
        Address::new("agent:margatroid/renamed:dev").unwrap(),
    );
    assert!(!world
        .address_exists("agent:margatroid/example:dev")
        .unwrap());
    assert_eq!(
        world
            .entity_by_address("agent:margatroid/renamed:dev")
            .unwrap(),
        second
    );
}

#[test]
fn despawn_and_index_reuse_preserve_full_entity_generation() {
    let mut world = World::new();
    let original = world.spawn();
    world.insert_component(
        original,
        Address::new("agent:margatroid/example:dev").unwrap(),
    );
    let held = world
        .entity_by_address("agent:margatroid/example:dev")
        .unwrap();
    assert!(world.despawn(original));
    assert!(matches!(
        world.entity_by_address("agent:margatroid/example:dev"),
        Err(AddressError::EntityMissing)
    ));
    let replacement = world.spawn();
    assert_eq!(held.index(), replacement.index());
    assert_ne!(held.generation(), replacement.generation());
    assert!(!world.is_alive(held));
    assert!(!world
        .address_exists("agent:margatroid/example:dev")
        .unwrap());
    world.insert_component(
        replacement,
        Address::new("agent:margatroid/example:dev").unwrap(),
    );
    assert_eq!(
        world
            .entity_by_address("agent:margatroid/example:dev")
            .unwrap(),
        replacement
    );
    assert!(world.get_component::<Address>(held).is_none());
}
