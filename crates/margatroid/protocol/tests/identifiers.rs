use margatroid_protocol::{ContentDigest, ProjectName, WorkspaceId};

#[test]
fn identifiers_are_validated_on_construction_and_deserialization() {
    let id = WorkspaceId::new("workspace-01").unwrap();
    assert_eq!(id.as_str(), "workspace-01");
    assert_eq!(serde_json::to_string(&id).unwrap(), r#""workspace-01""#);

    for invalid in ["", ".", "..", "with space", "path/name", "path\\name"] {
        assert!(WorkspaceId::new(invalid).is_err(), "accepted {invalid:?}");
    }

    assert!(serde_json::from_str::<ProjectName>(r#""bad/name""#).is_err());
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
