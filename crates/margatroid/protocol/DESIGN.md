# Protocol

## 类型
公开：
```text
ToolCall：领域工具调用，公开结构体
    id: String
    tool_name: String--内部值为AgentResourceMap内唯一的resource_name；不保存Provider临时名称
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

MessageDto::Error：错误展示消息DTO
    message: String--Agent创建成功后的轮次级稳定错误文本

ServerMessage::AgentMessageReasoningDelta：思考流式分片
    type: agent.message.reasoning_delta
    id: String--轮次ID
    agent: ResourceIdDto--稳定Agent ID
    content: String--仅包含本次新增的思考文本

ClientMessage::AgentVisibilityInject / AgentVisibilityRemove：前端默认资源可见性命令
    行为：使用WorkspaceReference和可选Agent资源ID路由，携带完整resource_id；AgentPlugin最终校验资源属于默认可见性

ClientMessage::AgentTurnAbort：前端中止当前Agent轮次命令
    行为：使用WorkspaceReference和可选Agent资源ID路由为RouteAgentTurnAbort；不由前端提供turn_id

ClientMessage::AgentAssistant：外部构造Assistant消息
    行为：仅用于显式手动资源调用；ToolCall携带resource_id而不是Agent内部tool_name，由AgentPlugin校验动态可见性并转换后交给Base Driver

ClientMessage::AgentWorkflowAttach / AgentWorkflowDetach：前端Workflow热插拔命令
    行为：第一阶段保留协议形状但转换后返回Unsupported；待MCL Workflow权限和消息订阅模型确定后实现

ClientMessage::MclCommand：外部MCL命令
    workspace: WorkspaceReference
    agent: Option<ResourceIdDto>--None表示manager
    command: String--一条完整MCL命令字符串
    binding: Option<serde_json::Value>--可选占位符绑定值
    行为：DtoPlugin解析协议，WorkspacePlugin解析目标Agent后发送MclCommandReceived；与Base Driver handle使用同一parser和事务执行器

ServerMessage::MclCommandResult：外部MCL命令回执
    id: String--复用请求ID
    result: Result<serde_json::Value, String>--成功命令值或稳定有界错误
    行为：只发送给发起请求的WebSocket连接，不广播给其他连接；等待Driver响应时不阻塞ECS主线程

AgentStateDto：后端Agent状态快照
    status: WorkspaceAgentStatusDto--creating、ready或failed
    working: bool--是否存在未结束的交互轮次；覆盖推理和工具调用阶段
    error: Option<String>--只有failed包含稳定错误，不包含路径、上下文或资源正文
    default_resources: Vec<ResourceIdDto>--用户可手动开关的Agent默认资源集合
    visible_resources: Vec<ResourceIdDto>
    mcl: Option<AgentMclStateDto>--Ready时包含Base、Plan与Workflow实例信息，Creating和Failed为空
    total_input_tokens: u64--历史Assistant响应累计输入Token
    total_output_tokens: u64--历史Assistant响应累计输出Token
    total_cache_hit_tokens: u64--历史Assistant响应累计缓存命中Token
    cache_hit_rate: f64--累计缓存命中率，等于total_cache_hit_tokens / total_input_tokens；总输入为0时为0
    last_input_tokens: u64--最近一条Assistant响应报告的输入Token
    context_window_tokens: u64--当前Agent模型的最大上下文窗口

WorkspaceAgentStatusDto：Workspace成员状态DTO
    Creating
    Ready
    Failed
```

## 逻辑
```text
InferencePlugin先把模型返回的Provider tool_name恢复为ResourceMapEntry.resource_name；ToolPlugin再根据消息所属Agent的AgentResourceMap恢复tool_id和resource_id。
resource_name只在单个Agent内唯一，可以是MCL alias或完整ResourceId字符串，不需要跨Agent全局唯一。
ToolPlugin从AgentResourceMap和Agent.tools.pending恢复具体resource_id并写入Message::Tool。
ResourceId统一格式为type:scope/name:tag，省略tag时解析为latest。
静态Workspace Agent固定使用agent:<workspace>/<name>:latest；clone tag不创建目录，动态Subagent留待后续设计。
agent.message.reasoning_delta与agent.message.delta分别累积思考和正文；完整agent.message同时结束两种分片。
BackendStateDto为Workspace定义中的每个Agent都生成AgentStateDto；Creating和Failed成员的working为false，default_resources与visible_resources为空且六项Token与窗口状态为0；只有Ready成员读取Agent Entity组件与AgentTokenUsage。
Ready成员的default_resources与visible_resources分别投影MCL的tool.tool_default与tool.tool_dynamic数组，MCL是唯一可见性事实源。
外部mcl.command一次只允许一条命令；不能通过协议上传或执行Lua源码，也不能取得World、Entity或Driver内部句柄。
AgentHistoryDto只为Ready且已经绑定AgentMemory的成员生成；成员创建状态变化和资源逐项注入或删除由下一次state.sync反映。
```
