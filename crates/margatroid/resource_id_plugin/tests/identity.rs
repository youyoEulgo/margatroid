use std::collections::BTreeMap;

use core_plugin::World;
use resource_id_plugin::{
    ResourceId, ResourceIdAttachError, ResourceIdLookupError, ResourceIdPluginInstalled,
    ResourceRef, WorldResourceIdExt,
};

#[test]
fn references_parse_normalize_and_serialize_as_map_keys() {
    let id = ResourceRef::parse("agent:local/coder").unwrap();
    assert_eq!(id.to_string(), "agent:local/coder:latest");
    assert_eq!(
        id,
        ResourceRef::new("agent", "local", "coder", None::<String>).unwrap()
    );
    assert_eq!(
        serde_json::from_str::<ResourceRef>("\"agent:local/coder\"").unwrap(),
        id
    );
    let map = BTreeMap::from([(id.clone(), 7)]);
    let json = serde_json::to_string(&map).unwrap();
    assert_eq!(json, "{\"agent:local/coder:latest\":7}");
    assert_eq!(
        serde_json::from_str::<BTreeMap<ResourceRef, i32>>(&json).unwrap(),
        map
    );
    for invalid in [
        "",
        "Agent:local/coder",
        "agent:../coder",
        "agent:local/a/b",
        "agent:local/a:x:y",
    ] {
        assert!(ResourceRef::parse(invalid).is_err(), "{invalid}");
        assert!(serde_json::from_value::<ResourceRef>(serde_json::json!(invalid)).is_err());
    }
}

#[test]
fn attachment_checks_duplicates_and_preserves_existing_identity() {
    let mut world = World::new();
    world.insert_resource(ResourceIdPluginInstalled);
    let first = world.spawn();
    let second = world.spawn();
    let id = ResourceRef::parse("agent:local/first").unwrap();
    let other = ResourceRef::parse("agent:local/second").unwrap();
    assert!(matches!(
        world.entity_by_resource_id(&id),
        Err(ResourceIdLookupError::Missing { .. })
    ));
    world.attach_resource_id(first, id.clone()).unwrap();
    world.attach_resource_id(first, id.clone()).unwrap();
    world.attach_resource_id(second, other.clone()).unwrap();
    assert!(matches!(
        world.attach_resource_id(second, id.clone()),
        Err(ResourceIdAttachError::Lookup(
            ResourceIdLookupError::Duplicate { .. }
        ))
    ));
    assert_eq!(world.entity_by_resource_id(&id), Ok(first));
    assert_eq!(world.entity_by_resource_id(&other), Ok(second));
    assert_eq!(
        world.get_component::<ResourceId>(second).unwrap().as_ref(),
        &other
    );
    world.insert_component(second, ResourceId(id.clone()));
    assert!(
        matches!(world.entity_by_resource_id(&id), Err(ResourceIdLookupError::Duplicate { entities, .. }) if entities == vec![first, second])
    );
    assert!(matches!(
        world.attach_resource_id(first, id.clone()),
        Err(ResourceIdAttachError::Lookup(
            ResourceIdLookupError::Duplicate { .. }
        ))
    ));
    world.despawn(second);
    assert!(matches!(
        world.attach_resource_id(second, other),
        Err(ResourceIdAttachError::MissingEntity { .. })
    ));
}

#[test]
fn attachment_requires_plugin_without_mutation() {
    let mut world = World::new();
    let entity = world.spawn();
    let id = ResourceRef::parse("agent:local/demo").unwrap();
    assert!(matches!(
        world.attach_resource_id(entity, id),
        Err(ResourceIdAttachError::Lookup(
            ResourceIdLookupError::PluginMissing
        ))
    ));
    assert!(world.get_component::<ResourceId>(entity).is_none());
}
