use address::{Address, AddressError, ParsedAddress};

#[test]
fn missing_tag_is_rejected_and_explicit_tag_is_preserved() {
    for kind in [
        "tool",
        "prompt",
        "sandbox",
        "driver",
        "workspace",
        "agent",
        "client",
        "execution",
    ] {
        assert!(ParsedAddress::parse(&format!("{kind}:margatroid/name")).is_err());
        assert!(Address::new(&format!("{kind}:margatroid/name")).is_err());
        assert!(Address::new(&format!("{kind}:margatroid/name:")).is_err());
    }
    for tag in ["dev", "v1.2", "stable"] {
        let value = format!("tool:margatroid/search:{tag}");
        let parsed = ParsedAddress::parse(&value).unwrap();
        assert_eq!(parsed.tag, tag);
        assert_eq!(parsed.to_string(), value);
        assert_eq!(Address::new(&value).unwrap().as_str(), value);
    }
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
        "tool:margatroid/name/extra",
        "tool:margatroid/name:v1/extra",
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
        ":scope/name:dev",
        "Tool:scope/name:dev",
        "tøøl:scope/name:dev",
        "tool.foo:scope/name:dev",
        "tool:scope/:v1",
        "tool:/name:dev",
        "tool:scope/name:",
        "tool:scope/name:tag:extra",
        "tool:s:cope/name:dev",
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
