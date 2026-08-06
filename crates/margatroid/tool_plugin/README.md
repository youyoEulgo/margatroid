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
Agent的默认或动态可见性组件。AgentPlugin使用当次临时名称映射把`ToolCall.name`解析为
`ResourceRef`，再通过`ToolCallRequest { id, agent, resource, call }`逐个派发。

ToolPlugin只解析请求中的单个`resource`，验证构造出的模型名称与`call.name`一致后异步执行handler。
成功或失败都返回`Message::Tool`形式的`AgentMessage`，意图固定为`ResolveToolCall`。这条执行链路不做
可见性权限检查；可见工具集合只在AgentPlugin构造请求时产生。
