use resource_id_plugin::{ResourceID, ResourceInner};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResource {
    pub alias: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub script_path: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptResource {
    pub alias: String,
    pub role: String,
    pub content: Arc<str>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxResource {
    pub alias: String,
    pub policy: Arc<str>,
}

macro_rules! resource_inner {
    ($($ty:ty),+) => {
        $(impl ResourceInner for $ty {
            fn get_alias(&self) -> &str { &self.alias }
        })+
    };
}
resource_inner!(ToolResource, PromptResource, SandboxResource);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDriverModelConfig {
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stop: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AgentDriver {
    pub base_driver: Arc<str>,
    pub origin: Arc<PathBuf>,
    pub dependencies: Arc<[resource_id_plugin::ResourceID]>,
    pub model: AgentDriverModelConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallableResource {
    pub resource_id: ResourceID,
    pub alias: String,
    pub definition: Arc<CallableDefinitionSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallableDefinitionSnapshot {
    Tool(ToolResource),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxValue {
    pub resource_id: ResourceID,
    pub policy: Arc<str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callable_snapshots_share_definitions_and_round_trip_tools() {
        let definitions = [(
            "tool",
            CallableDefinitionSnapshot::Tool(ToolResource {
                alias: "source".into(),
                description: "tool".into(),
                input_schema: serde_json::json!({"type": "object"}),
                script_path: "tool.lua".into(),
            }),
        )];
        for (kind, definition) in definitions {
            let value = CallableResource {
                resource_id: ResourceID::parse(&format!("{kind}:local/test")).unwrap(),
                alias: "imported".into(),
                definition: Arc::new(definition),
            };
            let cloned = value.clone();
            assert!(Arc::ptr_eq(&value.definition, &cloned.definition));
            let encoded = serde_json::to_string(&value).unwrap();
            assert_eq!(
                serde_json::from_str::<CallableResource>(&encoded).unwrap(),
                value
            );
        }
    }
}

#[cfg(test)]
mod rejected_legacy_snapshots {
    use super::*;

    #[test]
    fn legacy_definition_variants_are_rejected() {
        for kind in ["Skill", "Hook"] {
            let value = serde_json::json!({kind: {"alias":"legacy", "description":"legacy"}});
            assert!(serde_json::from_value::<CallableDefinitionSnapshot>(value).is_err());
        }
    }
}
