# MargatroidProtocol

## 类型

公开：
```text
ClientRequest：客户端发给daemon的请求，公开枚举--跨进程请求，不持有ECS Entity
    WorkspaceStart {
        id: String--调用方生成的Workspace启动请求ID
        definition: WorkspaceDefinitionDto--编译后的Workspace静态定义
    }
    AgentMessage {
        id: String--调用方生成的消息ID
        workspace: WorkspaceRefDto--目标Workspace逻辑身份
        agent: Option<String>--可选目标Agent名称，None表示使用Workspace.manager
        content: String--用户消息正文
    }
    impl Serialize + Deserialize for ClientRequest
        Serialize + Deserialize：使用type字段区分workspace.start和agent.message
    start_workspace(id: impl Into<String>, definition: &WorkspaceDefinition) -> Self
        构造Workspace启动请求：把领域定义转换为WorkspaceDefinitionDto
    agent_message(
        id: impl Into<String>,
        workspace: &WorkspaceRefDto,
        agent: Option<String>,
        content: impl Into<String>,
    ) -> Self
        构造用户消息请求：保留可选Agent名称，不在协议层决定manager

ServerEvent：daemon发给客户端的事件，公开枚举--运行状态、日志和Agent输出
    Log { record: LogRecordDto }--结构化daemon日志
    WorkspaceStarted { id: String, workspace: WorkspaceInfoDto }--Workspace启动完成
    AgentMessage { message: AgentMessageDto }--Agent产生的统一消息事件
    AgentFailure { failure: AgentFailureDto }--无法表示成Message的Agent轮次失败
    impl Serialize + Deserialize for ServerEvent
        Serialize + Deserialize：使用type字段区分log、workspace.started、agent.message和agent.failure

LogRecordDto：结构化日志记录，公开结构体--保存客户端展示所需的信息
    timestamp_millis: u64--日志时间
    level: String--日志级别
    target: String--日志目标
    message: String--日志正文
    fields: Vec<LogFieldDto>--结构化字段，缺省为空数组
    spans: Vec<String>--Tracing span名称，缺省为空数组
    impl Serialize + Deserialize for LogRecordDto

LogFieldDto：日志字段，公开结构体--保存结构化字段名称和值
    name: String--字段名称
    value: String--字段文本
    impl Serialize + Deserialize for LogFieldDto

WorkspaceRefDto：Workspace逻辑引用，公开结构体--跨进程定位已启动Workspace
    name: String--Workspace名称
    project_root: String--项目根路径文本，daemon按绝对路径规范化后查找
    new(name: impl Into<String>, project_root: impl Into<String>) -> Self
        构造引用：直接保存名称和路径文本
    from_definition(definition: &WorkspaceDefinition) -> Self
        从领域定义构造引用：复制名称并把路径转换为字符串
    impl Serialize + Deserialize for WorkspaceRefDto

WorkspaceInfoDto：Workspace启动信息，公开结构体--提供客户端选择Agent所需的信息
    name: String--Workspace名称
    project_root: String--项目根路径文本
    manager: String--默认Agent名称
    agents: Vec<String>--Workspace内全部Agent名称
    from_definition(definition: &WorkspaceDefinition) -> Self
        从领域定义构造启动信息：按定义顺序复制Agent名称
    reference(&self) -> WorkspaceRefDto
        取得逻辑引用：只保留名称和项目根路径
    impl Serialize + Deserialize for WorkspaceInfoDto

AgentMessageDto：Agent消息事件，公开结构体--携带daemon已解析的Workspace和Agent身份
    id: String--消息ID
    workspace: WorkspaceRefDto--消息所属Workspace
    agent: String--已解析的Agent名称
    message: margatroid_types::Message--统一Message内容
    impl Serialize + Deserialize for AgentMessageDto

AgentFailureDto：Agent失败事件，公开结构体--携带无法转为Message的轮次失败
    id: String--轮次或请求ID
    workspace: WorkspaceRefDto--失败所属Workspace
    agent: String--失败Agent名称
    kind: String--领域失败类型的协议文本
    message: String--有界失败描述
    impl Serialize + Deserialize for AgentFailureDto

WorkspaceDefinitionDto：Workspace跨进程定义，公开结构体--保存不含Entity的可序列化定义
    name: String--Workspace名称
    project_root: String--项目根路径文本
    manager: String--默认Agent名称
    agents: Vec<WorkspaceAgentDefinitionDto>--Agent定义，保持编译顺序
    from_definition(definition: &WorkspaceDefinition) -> Self
        转换定义：把领域定义转换为字符串DTO
    into_definition(self) -> Result<WorkspaceDefinition, ProtocolError>
        恢复定义：解析镜像和资源引用并构造领域定义
    impl Serialize + Deserialize for WorkspaceDefinitionDto

WorkspaceAgentDefinitionDto：Agent定义DTO，公开结构体--保存单个Agent的静态配置
    name: String--Agent名称
    image: String--scope/name[:tag]镜像引用文本
    resources: Vec<ResourceRefDto>--额外可见资源
    disable_resources: Vec<ResourceRefDto>--禁用资源
    memory_path: Option<String>--可选SQLite路径文本
    impl Serialize + Deserialize for WorkspaceAgentDefinitionDto

ResourceRefDto：资源引用DTO，公开结构体--保存Provider和资源名称
    provider: String--工具定义Plugin ID
    name: String--scope/name资源名称
    impl Serialize + Deserialize for ResourceRefDto

ProtocolErrorKind：协议恢复错误分类，公开枚举
    InvalidImageReference--AgentImage字符串无法恢复
    InvalidResourceReference--ResourceRef字符串无法恢复
    impl Clone + Copy + PartialEq + Eq for ProtocolErrorKind

ProtocolError：协议恢复错误，公开结构体--保存稳定分类和有界描述
    kind: ProtocolErrorKind--错误分类，私有
    message: String--错误描述，私有
    kind(&self) -> ProtocolErrorKind
        取得错误分类：返回稳定错误类型
    message(&self) -> &str
        取得错误描述：返回有界文本
    impl Clone + PartialEq + Eq for ProtocolError
    impl Display + Error for ProtocolError
```

## 函数

私有：
```text
WorkspaceAgentDefinitionDto::from_definition(definition: &WorkspaceAgentDefinition) -> Self
    转换Agent定义：将镜像、资源和Memory路径转换为字符串

WorkspaceAgentDefinitionDto::into_definition(self) -> Result<WorkspaceAgentDefinition, ProtocolError>
    恢复Agent定义：解析镜像、资源和Memory路径

ResourceRefDto::from_resource(resource: &ResourceRef) -> Self
    转换资源引用：把Provider和ResourceName写入DTO

ResourceRefDto::into_resource(self) -> Result<ResourceRef, ProtocolError>
    恢复资源引用：先解析ResourceName，再构造ResourceRef
```

## 逻辑

```text
Workspace启动：
    CLI编译Workspace文件
        -> ClientRequest::start_workspace
        -> serde_json序列化为workspace.start
        -> daemon反序列化ClientRequest
        -> WorkspaceDefinitionDto::into_definition
        -> 发送StartWorkspace给WorkspacePlugin
        -> 启动完成后发送workspace.started

Agent消息：
    客户端构造agent.message
        -> daemon按WorkspaceRefDto查询已注册Workspace
        -> agent存在时按名称查询WorkspaceAgent
        -> agent为None时查询Workspace.manager
        -> 构造margatroid_types::AgentMessage { Message::User, UserWithoutToolCalls }
        -> AgentPlugin处理上下文、工具和Inference
        -> daemon把内部AgentMessage转换为ServerEvent::AgentMessage

边界：
    Protocol不创建Entity、不读取YAML、不连接WebSocket、不查Workspace注册表
    Protocol不决定manager、不推断MessageIntent、不构造InferenceRequest
    daemon只负责把跨进程身份转换为内部Entity，具体消息处理属于AgentPlugin
```

## 持有关系

```text
ClientRequest
├── WorkspaceDefinitionDto
│   └── Vec<WorkspaceAgentDefinitionDto>
│       ├── Vec<ResourceRefDto>
│       ├── Vec<ResourceRefDto>
│       └── memory_path
└── AgentMessage
    └── WorkspaceRefDto

ServerEvent
├── LogRecordDto
├── WorkspaceInfoDto
├── AgentMessageDto
│   ├── WorkspaceRefDto
│   └── margatroid_types::Message
└── AgentFailureDto
    └── WorkspaceRefDto
```
