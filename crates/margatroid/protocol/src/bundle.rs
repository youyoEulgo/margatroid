use serde::{Deserialize, Serialize};

use crate::{ContentDigest, ResourceKind, SchemaVersion, WorkspaceSpec};

pub const RESOURCE_PACKAGE_FORMAT_VERSION: u32 = 1;
pub const SKILL_PACKAGE_MEDIA_TYPE: &str = "application/vnd.margatroid.skill+json";
pub const WORKFLOW_PACKAGE_MEDIA_TYPE: &str = "application/vnd.margatroid.workflow+json";

/// Canonical decoded representation of a bundled directory resource.
///
/// Producers must sort `files` by `path` and encode this value with compact
/// JSON. Package digests cover those exact JSON bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcePackage {
    pub format_version: u32,
    pub files: Vec<ResourcePackageFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcePackageFile {
    pub path: String,
    pub content_base64: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceManifestEntry {
    pub kind: ResourceKind,
    pub logical_name: String,
    pub format_version: u32,
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
