# Protocol

## 类型
公开：
```text
ToolCall：领域工具调用，公开结构体
    id: String
    tool_name: String--所属AgentToolMap内唯一的模型工具名
    arguments: String

ToolDefinition：模型侧工具定义，公开结构体
    name: String
    description: String
    input_schema: serde_json::Value

Message::Tool：工具结果消息，公开消息变体
    resource_id: ResourceId--本次调用对应的具体资源ID
    tool_call_id: String--对应ToolCall.id
    content: String--工具结果或稳定错误

ClientMessage::AgentSkillLoad / AgentSkillUnload / AgentSkillUnloadAll：前端持久Skill命令
    行为：使用WorkspaceReference和可选Agent资源ID路由；Load与Unload携带完整Skill resource_id

AgentStateDto：后端Agent状态快照
    visible_resources: Vec<ResourceIdDto>
    loading_skills: Vec<ResourceIdDto>--当前每轮自动调用的Skill集合
```

## 逻辑
```text
ToolCall保留模型返回的tool_name；ToolPlugin根据消息所属Agent的AgentToolMap恢复tool_id和resource_id。
tool_name只在单个Agent内唯一，不作为ResourceId，也不需要跨Agent全局唯一。
ToolPlugin从AgentToolMap和PendingToolCalls恢复具体resource_id并写入Message::Tool。
ResourceId统一格式为type:scope/name:tag，省略tag时解析为latest。
静态Workspace Agent固定使用agent:<workspace>/<name>:latest；clone tag不创建目录，动态Subagent留待后续设计。
```
