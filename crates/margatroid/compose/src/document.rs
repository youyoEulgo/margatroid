use std::collections::BTreeMap;

use serde::Deserialize;
use serde_yaml_ng::Value;

#[derive(Debug, Deserialize)]
pub(crate) struct ComposeDocument {
    pub schema_version: u32,
    pub workspace: WorkspaceDocument,
    pub agents: BTreeMap<String, AgentDocument>,
    #[serde(default)]
    pub volumes: BTreeMap<String, VolumeDocument>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkspaceDocument {
    pub name: Option<String>,
    pub description: Option<String>,
    pub manager: String,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AgentDocument {
    pub image: String,
    #[serde(default)]
    pub skills: Vec<ResourceDocument>,
    #[serde(default)]
    pub workflows: Vec<ResourceDocument>,
    pub memory_volume: Option<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct VolumeDocument {
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum ResourceDocument {
    Name(String),
    Detailed(ResourceDetailDocument),
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResourceDetailDocument {
    pub name: Option<String>,
    pub path: Option<String>,
    pub installed: Option<String>,
    pub expected_digest: Option<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

pub(crate) fn invalid_extension_keys<'a>(
    maps: impl IntoIterator<Item = (&'a str, &'a BTreeMap<String, Value>)>,
) -> Vec<String> {
    maps.into_iter()
        .flat_map(|(prefix, map)| {
            map.keys()
                .filter(|key| !key.starts_with("x-"))
                .map(move |key| {
                    if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    }
                })
        })
        .collect()
}
