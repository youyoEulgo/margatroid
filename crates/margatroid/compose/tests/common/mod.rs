use std::path::Path;
use tool_resource_core::{ToolEntry, ToolReadLimits, read_tool};

pub fn tool(path: impl AsRef<Path>) -> ToolEntry {
    let input = read_tool(path.as_ref(), &ToolReadLimits::default()).unwrap();
    ToolEntry {
        path: input.origin,
        description: input.description,
        input_schema: input.input_schema,
        owner_task_id: None,
        binding: None,
    }
}
