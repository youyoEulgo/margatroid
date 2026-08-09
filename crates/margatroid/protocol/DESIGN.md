# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# MargatroidProtocol

## 类型

公开：
```text
FromDomain<Domain, Context = ()>：领域值到DTO转换trait，公开trait--由DTO类型实现
    from_domain(domain: Domain, context: Context) -> Result<Self, ProtocolError>
        从领域值构造DTO：context提供转换需要的运行时信息，需要查询ECS时使用&World

IntoDomain<Domain, Context = ()>：DTO到领域值转换trait，公开trait--由DTO类型实现
    into_domain(self, context: Context) -> Result<Domain, ProtocolError>
        将DTO转换为领域值：context提供请求ID、连接ID或&World等转换上下文

IntoDto<Dto, Context = ()>：领域值转DTO便捷trait，公开trait--根据Dto: FromDomain<Self, Context>自动实现
    into_dto(self, context: Context) -> Result<Dto, ProtocolError>
        转换为DTO：调用Dto::from_domain(self, context)

FromDto<Dto, Context = ()>：DTO转领域值便捷trait，公开trait--根据Dto: IntoDomain<Self, Context>自动实现
    from_dto(dto: Dto, context: Context) -> Result<Self, ProtocolError>
        从DTO构造：调用dto.into_domain(context)

ClientMessage：客户端WebSocket请求，公开枚举--统一使用type、id和message三个顶层字段
    ConnectionRegister { id: String, message: RegisterConnectionDto }
        connection.register：声明当前连接的客户端类型
    WorkspaceStart { id: String, message: StartWorkspaceDto }
        workspace.start：提交编译后的Workspace定义
    WorkspaceStop { id: String, message: StopWorkspaceDto }
        workspace.stop：按Workspace名称和项目根请求停止已启动Workspace
    AgentMessage { id: String, message: RouteAgentMessageDto }
        agent.message：向逻辑Agent投递一条用户消息
    impl Serialize + Deserialize for ClientMessage
        Serialize + Deserialize：将type作为判别字段，将业务payload放入message字段
    register_connection(id: impl Into<String>, client_type: impl Into<String>) -> Self
        构造连接请求：构造统一信封，不校验连接类型
    start_workspace(id: impl Into<String>, definition: &WorkspaceDefinition) -> Self
        构造Workspace请求：使用StartWorkspaceDto保存可序列化定义
    agent_message(id: impl Into<String>, workspace: &WorkspaceReferenceDto, agent: Option<String>, content: impl Into<String>) -> Self
        构造Agent请求：构造只包含UserMessageDto的RouteAgentMessageDto

RegisterConnectionDto：连接注册DTO，公开结构体--保存客户端声明的连接类型
    client_type: String--客户端类型，例如webui或cli
    impl IntoDomain<RegisterConnection, (String, WebSocketConnectionId)> for RegisterConnectionDto
        转换连接命令：把协议字段、请求ID和当前连接转换为ConnectionPlugin领域事件

StartWorkspaceDto：Workspace启动DTO，公开结构体--保存StartWorkspace可序列化输入
    definition: WorkspaceDefinitionDto--Workspace静态定义
    impl IntoDomain<StartWorkspace, String> for StartWorkspaceDto
        转换Workspace命令：使用请求ID并调用WorkspaceDefinitionDto的IntoDomain实现

RouteAgentMessageDto：逻辑Agent消息DTO，公开结构体--保存尚未解析Entity的消息路由信息
    workspace: WorkspaceReferenceDto--目标Workspace逻辑引用
    agent: Option<String>--Agent逻辑名称，None表示manager
    message: UserMessageDto--用户消息内容，协议结构无法表达System、Assistant或Tool输入
    impl IntoDomain<RouteAgentMessage, String> for RouteAgentMessageDto
        转换消息命令：使用请求ID，将WorkspaceReferenceDto转换为WorkspaceReference并保留Message

WorkspaceDefinitionDto：Workspace定义DTO，公开结构体--保存不含Entity的可序列化定义
    name: String--Workspace名称
    project_root: String--项目根路径文本
    manager: String--默认Agent名称
    agents: Vec<WorkspaceAgentDefinitionDto>--Agent静态定义
    impl FromDomain<&WorkspaceDefinition> for WorkspaceDefinitionDto
        转换领域定义：将路径、镜像和资源转换为字符串
    impl IntoDomain<WorkspaceDefinition> for WorkspaceDefinitionDto
        转换领域值：解析镜像、资源和路径

WorkspaceReferenceDto：Workspace逻辑引用DTO，公开结构体--保存跨进程定位信息
    name: String--Workspace名称
    project_root: String--项目根路径文本
    impl FromDomain<&WorkspaceDefinition> for WorkspaceReferenceDto
        转换逻辑引用：从Workspace定义提取名称和项目根
    impl IntoDomain<WorkspaceReference> for WorkspaceReferenceDto
        转换逻辑引用：将project_root转换为PathBuf

StopWorkspaceDto：Workspace停止DTO，公开结构体--保存待停止Workspace的逻辑引用
    workspace: WorkspaceReferenceDto--Workspace名称和项目根
    impl IntoDomain<StopWorkspaceByReference, String> for StopWorkspaceDto
        转换停止命令：附加请求ID并转换WorkspaceReferenceDto

ServerMessage：daemon发给客户端的协议事件，公开枚举--统一使用type和业务字段
    Log { record: LogRecordDto }
    StateSync { state: BackendStateDto }
    WorkspaceStarted { id: String, workspace: WorkspaceInfoDto }
    WorkspaceStopped { id: String, workspace: WorkspaceReferenceDto }
        workspace.stopped：Workspace已停止，可作为客户端关闭连接的业务回执
    WorkspaceStopFailed { id: String, error: String }
        workspace.stop_failed：Workspace停止失败
    AgentMessage { message: AgentMessageDto }
    AgentFailure { failure: AgentFailureDto }
    impl Serialize + Deserialize for ServerMessage
        Serialize + Deserialize：使用type区分事件类型

UserMessageDto：用户输入DTO，公开结构体--只允许客户端提交用户文本
    content: String--用户消息正文
    impl IntoDomain<Message> for UserMessageDto
        转换用户消息：构造Message::User，不允许客户端选择其他领域Message变体

ToolCallDto：工具调用展示DTO，公开结构体--隔离领域ToolCall的协议表示
    id: String--工具调用ID
    name: String--外部工具名称
    arguments: String--JSON参数文本
    impl FromDomain<&ToolCall> for ToolCallDto
        转换工具调用：复制稳定外部字段

MessageDto：可展示消息DTO，公开枚举--只包含历史表和前端允许展示的消息类型
    User { content: String }
    Assistant { content: Option<String>, tool_calls: Vec<ToolCallDto> }
    impl FromDomain<&Message> for MessageDto
        转换可展示消息：User和Assistant成功；System和Tool返回UnsupportedMessage

AgentMessageDto：Agent消息展示DTO，公开结构体--把领域Agent Entity投影为稳定逻辑ID
    id: String--消息或turn ID
    workspace: WorkspaceReferenceDto--所属Workspace
    agent: String--AgentIdentity保存的稳定Agent逻辑ID
    message: MessageDto--隔离后的可展示消息正文
    impl FromDomain<&AgentMessage, &World> for AgentMessageDto
        转换展示消息：通过World将Agent Entity解析成稳定Agent ID

AgentFailureDto：Agent失败展示DTO，公开结构体--把领域失败投影为可展示文本
    id: String--轮次或请求ID
    workspace: WorkspaceReferenceDto--所属Workspace
    agent: String--Agent稳定逻辑ID
    kind: String--失败来源
    message: String--有界错误描述
    impl FromDomain<&AgentFailure, &World> for AgentFailureDto
        转换失败：解析稳定Agent ID与Workspace，并显式映射稳定kind字符串

LogRecordDto：结构化日志DTO，公开结构体--保存客户端展示所需的日志字段
    timestamp_millis: u64--日志时间
    level: String--日志级别
    target: String--日志目标
    message: String--日志正文
    fields: Vec<LogFieldDto>--结构化字段
    spans: Vec<String>--Tracing span名称

BackendStateDto：后端状态快照DTO，公开结构体--保存前端权威状态
    workspaces: Vec<WorkspaceInfoDto>--当前已就绪Workspace
    histories: Vec<AgentHistoryDto>--可展示历史
    impl FromDomain<(), &World> for BackendStateDto
        转换完整状态：查询全部Workspace、Agent和Memory历史并构造快照

WorkspaceAgentDefinitionDto：单Agent定义DTO，公开结构体--保存镜像、资源和Memory路径文本
ResourceRefDto：资源引用DTO，公开结构体--保存provider和scope/name文本
LogFieldDto：日志字段DTO，公开结构体--保存字段名和值
AgentHistoryDto：Agent历史DTO，公开结构体--保存Workspace引用、Agent逻辑名称和HistoryMessageDto列表
HistoryMessageDto：历史消息DTO，公开结构体--保存序号、turn ID、MessageDto、资源标记和时间
    impl FromDomain<HistoryMessage> for HistoryMessageDto
        转换历史条目：转换可展示Message和ResourceRef列表
```

## 逻辑

```text
统一入站信封：
    前端发送 { type, id, message }
        -> ServerPlugin::WebSocketMessageReceived
        -> DtoPlugin反序列化ClientMessage
        -> message DTO调用into_domain
        -> DtoPlugin直接发送领域事件

领域事件：
    workspace.start -> StartWorkspace
    workspace.stop -> StopWorkspaceByReference
    agent.message -> RouteAgentMessage
    connection.register -> RegisterConnection

Agent消息路由：
    RouteAgentMessage携带WorkspaceReference和Agent逻辑名称
        -> WorkspacePlugin查询Workspace
        -> WorkspaceAgents按逻辑名称取得Agent Entity
        -> 发送AgentMessage { agent: Entity }
        -> AgentPlugin直接使用Entity处理

出站：
    领域事件或状态
        -> 对应DTO调用FromDomain，必要时只读查询World
        -> DtoPlugin构造ServerMessage
        -> DtoPlugin序列化并发送WebSocket文本
```

## 边界

```text
Protocol负责：
    定义跨进程JSON形状
    定义所有XxxDto
    集中定义FromDomain、IntoDomain、IntoDto和FromDto
    由DTO类型实现与领域类型之间的信息隔离转换
    保证协议不暴露ECS Entity

Protocol不负责：
    修改Workspace、Agent或Memory状态
    生成Agent Entity
    决定manager
    执行消息意图、工具或推理

DtoPlugin负责：
    接收WebSocketMessageReceived
    解包统一信封
    调用DTO转换方法
    直接发送领域事件
    序列化ServerMessage并发送

DtoPlugin不再负责：
    发送只复制字段的API中间事件
    执行领域业务
```
