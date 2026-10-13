use resource_id_plugin::ResourceID;
use std::collections::BTreeMap;

#[test]
fn values_parse_normalize_and_serialize_as_map_keys() {
    let id = ResourceID::parse("agent:local/coder").unwrap();
    assert_eq!(id.to_string(), "agent:local/coder:latest");
    assert_eq!(
        id,
        ResourceID::new("agent", "local", "coder", None).unwrap()
    );
    assert_eq!(
        serde_json::from_str::<ResourceID>("\"agent:local/coder\"").unwrap(),
        id
    );
    let map = BTreeMap::from([(id.clone(), 7)]);
    let json = serde_json::to_string(&map).unwrap();
    assert_eq!(json, "{\"agent:local/coder:latest\":7}");
    assert_eq!(
        serde_json::from_str::<BTreeMap<ResourceID, i32>>(&json).unwrap(),
        map
    );
    for invalid in [
        "",
        "Agent:local/coder",
        "agent:../coder",
        "agent:local/a/b",
        "agent:local/a:x:y",
    ] {
        assert!(ResourceID::parse(invalid).is_err(), "{invalid}");
        assert!(serde_json::from_value::<ResourceID>(serde_json::json!(invalid)).is_err());
    }
}
