use margatroid_protocol::{AgentImageReference, ContentDigest, WorkspaceId, WorkspaceName};

#[test]
fn identifiers_are_validated_on_construction_and_deserialization() {
    let id = WorkspaceId::new("workspace-01").unwrap();
    assert_eq!(id.as_str(), "workspace-01");
    assert_eq!(serde_json::to_string(&id).unwrap(), r#""workspace-01""#);

    for invalid in ["", ".", "..", "with space", "path/name", "path\\name"] {
        assert!(WorkspaceId::new(invalid).is_err(), "accepted {invalid:?}");
    }

    assert!(serde_json::from_str::<WorkspaceName>(r#""bad/name""#).is_err());
}

#[test]
fn agent_image_references_require_scope_and_valid_version() {
    assert!(AgentImageReference::new("eulgo/coder:v1").is_ok());
    assert!(AgentImageReference::new(format!("eulgo/coder@sha256:{}", "a".repeat(64))).is_ok());
    assert!(AgentImageReference::new("coder:v1").is_err());
    assert!(AgentImageReference::new("eulgo/coder@sha256:bad").is_err());
    for invalid in [
        "eulgo/coder?debug",
        "eulgo/coder#latest",
        "eulgo/coder:v1?debug",
        "eulgo/-coder:v1",
        "eulgo/coder:\0",
    ] {
        assert!(
            AgentImageReference::new(invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
    assert!(AgentImageReference::new(format!("eulgo/{}", "a".repeat(256))).is_err());
}

#[test]
fn content_digest_requires_canonical_sha256() {
    let digest = format!("sha256:{}", "a".repeat(64));
    assert_eq!(
        ContentDigest::try_from(digest.as_str()).unwrap().as_str(),
        digest
    );

    assert!(ContentDigest::try_from("sha256:abcd").is_err());
    assert!(ContentDigest::try_from(format!("sha256:{}", "A".repeat(64))).is_err());
    assert!(ContentDigest::try_from(format!("md5:{}", "a".repeat(64))).is_err());
}
