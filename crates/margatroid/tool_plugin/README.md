# ToolPlugin

`ToolPlugin`统一构造所有模型可调用资源。普通Tool、Skill和Workflow都由
`ToolDefinitionProvider`把一个`ResourceRef`转换成完整`Tool`。

构造接口一次只接收一个资源：

```text
AgentPlugin遍历AgentDynamicVisibility.resources
-> ToolPlugin.resolve_tool(agent, &resource)
-> Tool
-> AgentPlugin收集ToolDefinition
-> InferenceCommand.tools
```

`AgentDynamicVisibility`及其资源集合只由AgentPlugin读取和遍历。ToolPlugin不接收该集合，也不读取
Agent的默认或动态可见性组件。AgentPlugin通过`ToolCallRequest`逐个派发前端指定或模型返回的
`ToolCall`；ToolPlugin完成后统一返回`Message::Tool`形式的`AgentMessage`。调用后的工具定位与执行
步骤后续单独设计。
