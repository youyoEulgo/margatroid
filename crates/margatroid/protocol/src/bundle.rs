use serde::{Deserialize, Serialize};

use crate::{ContentDigest, ResourceKind, SchemaVersion, WorkspaceSpec};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceManifestEntry {
    pub kind: ResourceKind,
    pub logical_name: String,
    pub digest: ContentDigest,
    pub size_bytes: u64,
    pub media_type: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceManifest {
    #[serde(default)]
    pub entries: Vec<ResourceManifestEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundledResource {
    pub digest: ContentDigest,
    pub content_base64: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceBundle {
    pub schema_version: SchemaVersion,
    pub spec: WorkspaceSpec,
    pub manifest: ResourceManifest,
    #[serde(default)]
    pub resources: Vec<BundledResource>,
}
