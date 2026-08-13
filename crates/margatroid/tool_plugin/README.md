# ToolPlugin

ToolPlugin保存通用Loader和静态工具的`ToolTemplate`，并把定义检查与领域工具调用路由给具体工具Plugin。

```text
ToolDefinitionRequest -> ToolDefinitionRoute -> ToolDefinitionResult
ToolCallRequest -> ToolCallEvent -> AgentMessage::Tool
```

`ToolCall`直接保存完整`ResourceId`。ToolPlugin只检查对应模板是否注册，不读取Skill或Workflow文件，
不解析工具参数，不执行handler，也不检查Agent可见性。

`AgentToolEnvironment`只保存项目根和镜像根，供具体资源Plugin按项目、镜像、主目录顺序查找资源。
