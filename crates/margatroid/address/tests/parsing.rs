use address::{Address, AddressError, ParsedAddress};

#[test]
fn missing_tag_normalizes_and_explicit_tag_is_preserved() {
    let parsed = ParsedAddress::parse("tool:local/search").unwrap();
    assert_eq!(parsed.kind, "tool");
    assert_eq!(parsed.scope, "local");
    assert_eq!(parsed.name, "search");
    assert_eq!(parsed.tag, "latest");
    assert_eq!(parsed.to_string(), "tool:local/search:latest");
    assert_eq!(
        Address::new("tool:local/search").unwrap(),
        Address::new("tool:local/search:latest").unwrap()
    );
    assert_eq!(
        Address::new("tool:local/search:v1.2").unwrap().as_str(),
        "tool:local/search:v1.2"
    );
}

#[test]
fn field_content_is_preserved_without_extra_tag_restrictions() {
    let value = "custom_kind-2:作用域/Some name:标签 beta";
    assert_eq!(ParsedAddress::parse(value).unwrap().to_string(), value);
    assert_eq!(Address::new(value).unwrap().as_str(), value);
}

#[test]
fn malformed_structures_are_rejected() {
    for value in [
        "",
        "tool",
        "tool:local",
        "tool:local/name/extra",
        "tool:local/name:v1/extra",
    ] {
        assert!(
            matches!(
                ParsedAddress::parse(value),
                Err(AddressError::InvalidFormat)
            ),
            "{value:?}"
        );
    }
}

#[test]
fn invalid_parts_and_path_traversal_are_rejected_in_every_field() {
    for value in [
        ":scope/name",
        "Tool:scope/name",
        "tøøl:scope/name",
        "tool.foo:scope/name",
        "tool:scope/:v1",
        "tool:/name",
        "tool:scope/name:",
        "tool:scope/name:tag:extra",
        "tool:s:cope/name",
    ] {
        assert!(
            matches!(ParsedAddress::parse(value), Err(AddressError::InvalidPart)),
            "{value:?}"
        );
    }
    for invalid in [".", "..", "a\\b", "a\nb", "a\0b", "a\u{7f}b"] {
        for value in [
            format!("tool:{invalid}/name:tag"),
            format!("tool:scope/{invalid}:tag"),
            format!("tool:scope/name:{invalid}"),
        ] {
            assert!(ParsedAddress::parse(&value).is_err(), "{value:?}");
            assert!(Address::new(&value).is_err(), "{value:?}");
        }
    }
}
