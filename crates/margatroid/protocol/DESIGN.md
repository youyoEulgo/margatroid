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

MessageDto::Assistant：Assistant展示消息DTO
    reasoning: Option<String>--Provider公开的完整思考内容
    content: Option<String>--Assistant正文
    tool_calls: Vec<ToolCallDto>

ServerMessage::AgentMessageReasoningDelta：思考流式分片
    type: agent.message.reasoning_delta
    id: String--轮次ID
    agent: ResourceIdDto--稳定Agent ID
    content: String--仅包含本次新增的思考文本

ClientMessage::AgentSkillLoad / AgentSkillUnload / AgentSkillUnloadAll：前端持久Skill命令
    行为：使用WorkspaceReference和可选Agent资源ID路由；Load与Unload携带完整Skill resource_id

ClientMessage::AgentVisibilityInject / AgentVisibilityRemove：前端默认资源可见性命令
    行为：使用WorkspaceReference和可选Agent资源ID路由，携带完整resource_id；AgentPlugin最终校验资源属于默认可见性

ClientMessage::AgentTurnAbort：前端中止当前Agent轮次命令
    行为：使用WorkspaceReference和可选Agent资源ID路由为RouteAgentTurnAbort；不由前端提供turn_id

AgentStateDto：后端Agent状态快照
    status: WorkspaceAgentStatusDto--creating、ready或failed
    working: bool--是否存在未结束的交互轮次；覆盖推理和工具调用阶段
    error: Option<String>--只有failed包含稳定错误，不包含路径、上下文或资源正文
    default_resources: Vec<ResourceIdDto>--用户可手动开关的Agent默认资源集合
    visible_resources: Vec<ResourceIdDto>
    loading_skills: Vec<ResourceIdDto>--当前每轮自动调用的Skill集合

WorkspaceAgentStatusDto：Workspace成员状态DTO
    Creating
    Ready
    Failed
```

## 逻辑
```text
ToolCall保留模型返回的tool_name；ToolPlugin根据消息所属Agent的AgentToolMap恢复tool_id和resource_id。
tool_name只在单个Agent内唯一，不作为ResourceId，也不需要跨Agent全局唯一。
ToolPlugin从AgentToolMap和PendingToolCalls恢复具体resource_id并写入Message::Tool。
ResourceId统一格式为type:scope/name:tag，省略tag时解析为latest。
静态Workspace Agent固定使用agent:<workspace>/<name>:latest；clone tag不创建目录，动态Subagent留待后续设计。
agent.message.reasoning_delta与agent.message.delta分别累积思考和正文；完整agent.message同时结束两种分片。
BackendStateDto为Workspace定义中的每个Agent都生成AgentStateDto；Creating和Failed成员的working为false，default_resources、visible_resources与loading_skills为空；只有Ready成员读取Agent Entity组件。
AgentHistoryDto只为Ready且已经绑定AgentMemory的成员生成；成员创建状态变化和资源逐项注入或删除由下一次state.sync反映。
```
