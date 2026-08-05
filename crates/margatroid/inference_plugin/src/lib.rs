use core_plugin::Component;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentToolDefinitions {
    tools: Vec<ToolDefinition>,
}

impl AgentToolDefinitions {
    pub fn new(tools: Vec<ToolDefinition>) -> Self {
        Self { tools }
    }

    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }
}

impl Component for AgentToolDefinitions {}
