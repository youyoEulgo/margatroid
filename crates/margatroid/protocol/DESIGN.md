# Protocol

## 类型
公开：
```text
ToolCall：领域工具调用，公开结构体
    id: String
    tool_name: String--内部值为AgentResourceMap内唯一的resource_name；不保存Provider临时名称
    arguments: String

归属说明：ToolDefinition 定义在 types crate，由 margatroid_types 重新导出使用；
    protocol 只引用它作为消息与请求的组成部分，不拥有其定义。

Message::Tool：工具结果消息，公开消息变体
    resource_id: ResourceId--本次调用对应的具体资源ID
    tool_call_id: String--对应ToolCall.id
    content: String--工具结果或稳定错误

MessageDto::Assistant：Assistant展示消息DTO，客户端可注入的两种方向之一
    reasoning: Option<String>--Provider公开的完整思考内容
    content: Option<String>--Assistant正文
    tool_calls: Vec<ToolCallDto>

MessageDto::Error：错误展示消息DTO
    边界：MessageDto同时用于出站投影与入站注入；入站方向只接受User与Assistant，Tool与Error返回InvalidRequest
    message: String--Agent创建成功后的轮次级稳定错误文本

ServerMessage::ConnectionRegistered：注册成功回执
    type: connection.registered
    id: String--原样回显请求ID
    client: ClientInfoDto--分配到的身份

ServerMessage::ConnectionRegisterFailed：注册失败回执
    type: connection.register_failed
    id: String--原样回显请求ID
    error: String--ClientError的稳定文本

ClientInfoDto：客户端身份DTO，公开结构体--resource_id、client_type与name

ServerMessage::AgentMessageReasoningDelta：思考流式分片
    type: agent.message.reasoning_delta
    id: String--轮次ID
    agent: ResourceIdDto--稳定Agent ID
    content: String--仅包含本次新增的思考文本

ClientMessage::AgentVisibilityInject / AgentVisibilityRemove：前端默认资源可见性命令
    行为：使用WorkspaceReference和可选Agent资源ID路由，携带完整resource_id；AgentPlugin最终校验资源属于默认可见性

ClientMessage::AgentTurnAbort：前端中止当前Agent轮次命令
    行为：使用WorkspaceReference和可选Agent资源ID路由为RouteAgentTurnAbort；不由前端提供turn_id

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

ProtocolError：协议转换错误，公开结构体
    kind: ProtocolErrorKind--稳定有限分类
    message: String--稳定有界描述
    kind(&self) -> ProtocolErrorKind
        读取分类：公开方法
    message(&self) -> &str
        读取描述：公开方法
    new(kind: ProtocolErrorKind, message: impl Into<String>) -> Self
        构造：公开关联函数

ProtocolErrorKind：协议错误分类，公开枚举
    ClientNotFound--按身份定位不到客户端
    AgentNotFound--按身份定位不到Agent
    WorkspaceNotFound--按身份定位不到Workspace
    MemoryNotFound--Agent缺少记忆
    MemoryReadFailed--历史读取失败
    HistoryEntryInvalid--历史条目不是可展示消息
    UnsupportedMessage--该消息变体不对外可见，例如System
    InvalidRequest--请求形状或方向非法
    InvalidResourceName--资源ID非法
    unsupported--返回Unsupported的错误
    result(&self) -> Result<T, ProtocolError>
        成功包装：公开方法，把值包成Ok，用于转换链收尾

FromDto / IntoDto：入站转换trait，公开
    语义：把DTO转换为领域值，转换结果携带错误分类
    实现：ClientMessage到领域事件、RegisterConnectionDto到RegisterConnection等

FromDomain / IntoDomain：出站转换trait，公开
    语义：把领域值投影为DTO；描述、参数、正文原样传递，不追加业务判断
    实现：AgentMessage、AgentFailure、HistoryMessage、WorkspaceInfo、AgentState等

MessageDto / AgentMessageDto / AgentHistoryDto / AgentResourceDto / AgentStateDto / BlockPathDto：
    出站投影与入站注入共用的DTO族
    边界：MessageDto同时用于两个方向；入站只接受User、Assistant与Inject，
          Tool与Error在顶层返回InvalidRequest，Inject内层则全部接受且不允许嵌套

MessageDto::Inject：上下文注入DTO--客户端请求记录上下文而不直写driver的Block
    messages: Vec<MessageDto>--需要记录的消息，非空且不含嵌套注入

MessageDto::Tool：工具结果DTO
    resource_id: ResourceIdDto
    tool_call_id: String
    content: String
    failed: bool--本次调用是否失败，缺省为false

LogRecordDto / LogFieldDto：日志出站投影
    LogRecordDto：时间戳、级别、目标、消息与字段列表
    LogFieldDto：字段名与字段值

MclCommandDto：外部MCL命令DTO
    workspace: WorkspaceReferenceDto
    agent: Option<ResourceIdDto>
    command: String
    binding: Option<serde_json::Value>

AgentMclStateDto：Agent的MCL实例信息DTO
    base: ResourceIdDto
    base_program_hash: String
    plan_hash: String
    plan_generation: u64

AgentFailureDto：Agent失败事件DTO
    id: String
    workspace: WorkspaceReferenceDto
    agent: ResourceIdDto
    kind: String--agent、inference或tool
    message: String

HistoryMessageDto：可展示历史条目DTO
    sequence: i64
    turn_id: String
    message: MessageDto
    created_at_ms: i64

WorkspaceReferenceDto：Workspace逻辑引用DTO，公开结构体
    id: String
    name: String
    project_root: PathBuf
    with_id(id: String, name: String, project_root: PathBuf) -> WorkspaceReferenceDto
        按显式ID构造：公开关联函数

    reference(&self) -> WorkspaceReferenceDto
        WorkspaceInfoDto的公开方法：从Workspace信息取出引用

WorkspaceInfoDto：Workspace信息DTO，公开结构体
    id: String--Workspace资源ID
    name: String--Workspace名
    project_root: PathBuf--项目根路径
    manager: ResourceIdDto--manager Agent
    agents: Vec<ResourceIdDto>--成员Agent

WorkspaceAgentDefinitionDto / WorkspaceDefinitionDto：Workspace定义DTO
    WorkspaceAgentDefinitionDto：Agent名与镜像引用
    WorkspaceDefinitionDto：名称、项目根、manager、agents与memory_path

StartWorkspaceDto / StopWorkspaceDto：Workspace生命周期请求DTO
    StartWorkspaceDto：携带完整WorkspaceDefinitionDto
    StopWorkspaceDto：携带Workspace引用

RouteAgentMessageDto / RouteAgentTargetDto：路由请求DTO
    RouteAgentMessageDto：Workspace引用、可选Agent与MessageDto
    RouteAgentTargetDto：Workspace引用与可选Agent，用于中止与目标解析

ServerMessage::AgentFailure：Agent失败出站事件
    type: agent.failure
    failure: AgentFailureDto

ServerMessage::StateSync：完整后端状态快照
    type: state.sync
    state: BackendStateDto--workspaces、agents与histories三段

BackendStateDto：后端状态快照DTO，公开结构体
    workspaces: Vec<WorkspaceInfoDto>
    agents: Vec<AgentStateDto>
    histories: Vec<AgentHistoryDto>
```

## 逻辑
```text
ClientMessage::register_connection / register_connection_with_name / start_workspace / stop_workspace / agent_message
    请求构造：公开关联函数，按字段拼出对应的ClientMessage变体，供CLI与测试使用
    register_connection：只给client_type，name缺省
    register_connection_with_name：显式指定name
    start_workspace：从WorkspaceDefinition投影出StartWorkspaceDto
    stop_workspace：按Workspace引用构造StopWorkspaceDto
    agent_message：按Workspace引用、可选Agent与消息构造RouteAgentMessageDto

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
