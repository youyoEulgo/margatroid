use anyhow::{Ok, Result};
use rmcp::{
    ServiceExt, model::CallToolRequestParams, service::RunningService, transport::TokioChildProcess,
};
use std::collections::HashMap;
use tokio::process::Command;
use types::mcp::{McpContent, McpToolCall, McpToolDef, McpToolResult};

pub struct McpClient {
    service: RunningService<rmcp::RoleClient, ()>,
}

impl McpClient {
    pub async fn connect_stdio(command: &str, args: &[&str]) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        let transport = TokioChildProcess::new(cmd)?;
        let service = ().serve(transport).await?;
        Ok(Self { service })
    }

    pub async fn connect_http(_url: &str) -> Result<Self> {
        todo!("rmcp HTTP SSE client connect")
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let resp = self.service.peer().list_tools(Default::default()).await?;
        let tools = resp
            .tools
            .into_iter()
            .map(|t| McpToolDef {
                name: t.name.to_string(),
                description: t.description.map(|d| d.to_string()),
                input_schema: serde_json::to_value(&t.input_schema).unwrap_or_default(),
            })
            .collect();
        Ok(tools)
    }

    pub async fn call_tool(&self, req: McpToolCall) -> Result<McpToolResult> {
        let params = match req.arguments {
            serde_json::Value::Object(m) => CallToolRequestParams::new(req.name).with_arguments(m),
            _ => CallToolRequestParams::new(req.name),
        };
        let resp = self.service.peer().call_tool(params).await?;
        let content = resp
            .content
            .into_iter()
            .filter_map(|c| {
                let v = serde_json::to_value(&c).ok()?;
                serde_json::from_value::<McpContent>(v).ok()
            })
            .collect();
        Ok(McpToolResult {
            content,
            is_error: resp.is_error,
        })
    }
}

pub struct McpClientPool {
    clients: HashMap<String, McpClient>,
}

impl McpClientPool {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }
    pub fn add(&mut self, name: impl Into<String>, client: McpClient) {
        self.clients.insert(name.into(), client);
    }
    pub async fn all_tools(&self) -> Result<Vec<McpToolDef>> {
        let mut result = Vec::new();
        for client in self.clients.values() {
            result.extend(client.list_tools().await?);
        }
        Ok(result)
    }
    pub async fn dispatch(&self, req: McpToolCall) -> Result<McpToolResult> {
        for client in self.clients.values() {
            let tools = client.list_tools().await?;
            if tools.iter().any(|t| t.name == req.name) {
                return client.call_tool(req).await;
            }
        }
        anyhow::bail!("no MCP server handles tool: {}", req.name)
    }
}
