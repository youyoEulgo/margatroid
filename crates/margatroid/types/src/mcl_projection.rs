use crate::block_types::BlockInner;
use crate::mcl_events::MclValue;
use crate::resource_components::CallableDefinitionSnapshot;
use serde_json::{json, Value};

pub fn project_mcl_value(value: &MclValue) -> Result<Value, serde_json::Error> {
    match value {
        MclValue::Array(BlockInner::Tool(resources)) => {
            let values = resources.iter().map(|resource| {
                let (description, schema) = match resource.definition.as_ref() {
                    CallableDefinitionSnapshot::Tool(definition) => (&definition.description, definition.input_schema.clone()),
                };
                json!({ "resource_id": resource.resource_id, "alias": resource.alias, "description": description, "input_schema": schema })
            }).collect::<Vec<_>>();
            Ok(json!({ "Array": { "Tool": values } }))
        }
        MclValue::Array(BlockInner::Sandbox(resources)) => {
            let values = resources
                .iter()
                .map(|resource| json!({ "resource_id": resource.resource_id }))
                .collect::<Vec<_>>();
            Ok(json!({ "Array": { "Sandbox": values } }))
        }
        _ => serde_json::to_value(value),
    }
}
