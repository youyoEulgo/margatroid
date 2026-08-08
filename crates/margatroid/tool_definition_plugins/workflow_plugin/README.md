# WorkflowPlugin

WorkflowPlugin注册`provider="workflow"`的`ToolDefinitionProvider`。每个可见Workflow都是一个独立
Tool，与普通Tool和Skill共享由AgentPlugin驱动的构造链路。

当前是占位实现。它按项目目录、AgentImage、主目录的顺序确认
`workflows/<scope>/<name>/` 存在，使可见 Workflow 能正常进入 LLM 工具定义；它不读取 Workflow
正文、不修改 Agent 状态，也不执行任何步骤。模型调用时只返回“尚未实现”的 Tool 响应，以便对话链
继续回到 inference。

```rust
use workflow_plugin::WorkflowPlugin;

let workflows = WorkflowPlugin::open("/home/user/.margatroid/workflows")?;
app.add_plugin(tool_plugin::ToolPlugin::default())
    .add_plugin(workflows);
# Ok::<(), Box<dyn std::error::Error>>(())
```
