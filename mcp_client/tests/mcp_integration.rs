//! 集成测试：McpClient ↔ margatroid-mcp-server（stdio transport）
//!
//! 运行前需要编译好 margatroid-mcp-server：
//!   cd /tmp/margatroid_mcp_server && cargo build --release
//!
//! 测试包含两个层次：
//!   1. 纯连通性：list_tools + call_tool
//!   2. AI 参与：让 AI 决定调用哪个 tool，验证结果

use anyhow::Result;
use mcp_client::client::McpClient;
use providers;
use types::mcp::{McpContent, McpToolCall};

/// margatroid-mcp-server 二进制路径
/// 可通过环境变量覆盖
fn server_bin() -> String {
    std::env::var("MARGATROID_MCP_SERVER_BIN")
        .unwrap_or_else(|_| "/tmp/margatroid_mcp_server/target/release/margatroid-mcp-server".into())
}

// ─── 1. 纯连通性测试 ──────────────────────────────────────

#[tokio::test]
async fn test_list_tools() -> Result<()> {
    let bin = server_bin();
    let client = McpClient::connect_stdio(&bin, &[]).await?;

    let tools = client.list_tools().await?;

    assert!(!tools.is_empty(), "should have at least one tool");
    let echo = tools.iter().find(|t| t.name == "echo");
    assert!(echo.is_some(), "should have 'echo' tool");

    let echo = echo.unwrap();
    assert_eq!(
        echo.description.as_deref(),
        Some("Echo back the input message")
    );

    println!("tools: {tools:#?}");
    Ok(())
}

#[tokio::test]
async fn test_call_echo() -> Result<()> {
    let bin = server_bin();
    let client = McpClient::connect_stdio(&bin, &[]).await?;

    let result = client
        .call_tool(McpToolCall {
            name: "echo".into(),
            arguments: serde_json::json!({ "message": "hello from test" }),
        })
        .await?;

    assert!(
        result.is_error != Some(true),
        "should not be an error, got: {:?}",
        result.is_error
    );
    assert!(!result.content.is_empty(), "should have content");

    let text = result.content.iter().find_map(|c| match c {
        McpContent::Text { text } => Some(text.clone()),
        _ => None,
    });

    assert_eq!(
        text.as_deref(),
        Some("hello from test"),
        "echo should return the same message"
    );

    println!("echo result: {result:#?}");
    Ok(())
}

// ─── 2. AI 参与的端到端测试 ───────────────────────────────
//
// 需要 OPENROUTER_API_KEY 环境变量
// 没有 key 时自动跳过

#[tokio::test]
async fn test_ai_driven_tool_call() -> Result<()> {
    let api_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("OPENROUTER_API_KEY not set, skipping AI test");
            return Ok(());
        }
    };

    // 1. 启动 MCP server，获取 tool 定义
    let bin = server_bin();
    let client = McpClient::connect_stdio(&bin, &[]).await?;
    let tools = client.list_tools().await?;

    // 2. 把 MCP tool 转成 ChatRequest 的 tools 格式
    let request_tools: Vec<types::RequestTool> = tools
        .iter()
        .map(|t| types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect();

    // 3. 让 AI 决定调用哪个 tool
    use providers::AiProvider;
    use types::{
        ChatRequest, RequestMessage,
        message::{ChatMessage, MessageContent, Role},
    };

    let provider = providers::OpenRouterProvider::new(api_key);

    let req = ChatRequest {
        model: "openai/gpt-4o-mini".into(),
        messages: vec![RequestMessage::Chat(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(
                "请用 echo tool 把这条消息原样返回：「MCP连通测试成功」".into(),
            ),
            name: None,
            tool_calls: None,
            reasoning_content: None,
        })],
        tools: Some(request_tools),
        tool_choice: Some(types::RequestToolChoice::String("required".into())),
        ..Default::default()
    };

    let resp = provider.chat(req).await?;
    println!("AI response: {resp:#?}");

    // 4. 解析 AI 的 tool_call 决策
    let choice = resp.choices.first().expect("no choices");
    let tool_calls = choice
        .message
        .tool_calls
        .as_ref()
        .expect("AI should have made a tool call");

    assert!(!tool_calls.is_empty(), "should have at least one tool call");

    let tc = &tool_calls[0];
    assert_eq!(tc.function.name, "echo", "AI should call 'echo'");

    // 5. 执行 AI 决定的 tool call
    let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)?;
    let mcp_result = client
        .call_tool(McpToolCall {
            name: tc.function.name.clone(),
            arguments: args,
        })
        .await?;

    println!("MCP result: {mcp_result:#?}");

    // 6. 验证结果
    assert!(
        mcp_result.is_error != Some(true),
        "MCP call failed: {:?}",
        mcp_result.content
    );
    let text = mcp_result.content.iter().find_map(|c| match c {
        McpContent::Text { text } => Some(text.clone()),
        _ => None,
    });

    assert!(
        text.as_deref().unwrap_or("").contains("MCP连通测试成功"),
        "echo result should contain the original message, got: {text:?}"
    );

    println!("✅ AI-driven MCP test passed: {text:?}");
    Ok(())
}
