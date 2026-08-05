# ToolPlugin

`ToolPlugin` 负责注册完整工具、为 Agent 创建可见工具快照，并把 Provider 工具调用异步执行为
可路由的结果。具体工具由独立 Plugin 提供，注册到进程不代表任何 Agent 自动可见。

```rust
use std::convert::Infallible;

use inference_plugin::ToolDefinition;
use margatroid_types::ResourceName;
use serde::Deserialize;
use serde_json::json;
use tool_plugin::{AppToolExt, Tool, ToolContext};

#[derive(Deserialize)]
struct EchoArguments {
    text: String,
}

let echo = Tool::new(
    ResourceName::new("builtin/echo")?,
    ToolDefinition {
        name: "echo".into(),
        description: "Return the supplied text".into(),
        input_schema: json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        }),
    },
    |_context: ToolContext, arguments: EchoArguments| async move {
        Ok::<_, Infallible>(arguments.text)
    },
)?;

app.register_tool(echo);
```

为 Agent 选择已注册工具：

```rust
use margatroid_types::ResourceName;
use tool_plugin::WorldToolExt;

world.set_registered_agent_tools(
    agent,
    [ResourceName::new("builtin/echo")?],
)?;
```

执行入口是 `WorldToolExt::send_tool_call`，结果通过 `ToolCallResult` Event 返回。结果始终保留
Agent Entity、请求 ID 和 Provider tool-call ID。ToolPlugin 不修改消息历史，也不负责后续
tool-call loop。
