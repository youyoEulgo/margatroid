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
ClientRequest：客户端WebSocket请求，公开枚举--统一使用type、id和message三个顶层字段
    ConnectionRegister { id: String, message: RegisterConnectionDto }
        connection.register：声明当前连接的客户端类型
    WorkspaceStart { id: String, message: StartWorkspaceDto }
        workspace.start：提交编译后的Workspace定义
    AgentMessage { id: String, message: RouteAgentMessageDto }
        agent.message：向逻辑Agent投递一条用户消息
    impl Serialize + Deserialize for ClientRequest
        Serialize + Deserialize：将type作为判别字段，将业务payload放入message字段
    register_connection(id: impl Into<String>, client_type: impl Into<String>) -> Self
        构造连接请求：构造统一信封，不校验连接类型
    start_workspace(id: impl Into<String>, definition: &WorkspaceDefinition) -> Self
        构造Workspace请求：使用StartWorkspaceDto保存可序列化定义
    agent_message(id: impl Into<String>, message: RouteAgentMessageDto) -> Self
        构造Agent请求：使用RouteAgentMessageDto作为message字段

RegisterConnectionDto：连接注册DTO，公开结构体--保存客户端声明的连接类型
    client_type: String--客户端类型，例如webui或cli
    into_domain(self, id: String, connection_id: WebSocketConnectionId) -> RegisterConnection
        转换连接命令：把协议字段和当前连接上下文转换为ConnectionPlugin领域事件

StartWorkspaceDto：Workspace启动DTO，公开结构体--保存StartWorkspace可序列化输入
    definition: WorkspaceDefinitionDto--Workspace静态定义
    into_domain(self, id: String) -> Result<StartWorkspace, ProtocolError>
        转换Workspace命令：调用WorkspaceDefinitionDto::into_domain并构造StartWorkspace

RouteAgentMessageDto：逻辑Agent消息DTO，公开结构体--保存尚未解析Entity的消息路由信息
    workspace: WorkspaceReferenceDto--目标Workspace逻辑引用
    agent: Option<String>--Agent逻辑名称，None表示manager
    message: margatroid_types::Message--消息内容，当前入口只接受User变体
    into_domain(self, id: String) -> RouteAgentMessage
        转换消息命令：将WorkspaceReferenceDto转换为WorkspaceReference并保留Message

WorkspaceDefinitionDto：Workspace定义DTO，公开结构体--保存不含Entity的可序列化定义
    name: String--Workspace名称
    project_root: String--项目根路径文本
    manager: String--默认Agent名称
    agents: Vec<WorkspaceAgentDefinitionDto>--Agent静态定义
    from_domain(definition: &WorkspaceDefinition) -> Self
        转换领域定义：将路径、镜像和资源转换为字符串
    into_domain(self) -> Result<WorkspaceDefinition, ProtocolError>
        转换领域值：解析镜像、资源和路径

WorkspaceReferenceDto：Workspace逻辑引用DTO，公开结构体--保存跨进程定位信息
    name: String--Workspace名称
    project_root: String--项目根路径文本
    into_domain(self) -> Result<WorkspaceReference, ProtocolError>
        转换逻辑引用：将project_root转换为PathBuf

ServerEvent：daemon发给客户端的协议事件，公开枚举--统一使用type和业务字段
    Log { record: LogRecordDto }
    StateSync { state: BackendStateDto }
    WorkspaceStarted { id: String, workspace: WorkspaceInfoDto }
    AgentMessage { message: AgentMessageDto }
    AgentFailure { failure: AgentFailureDto }
    impl Serialize + Deserialize for ServerEvent
        Serialize + Deserialize：使用type区分事件类型

AgentMessageDto：Agent消息展示DTO，公开结构体--把领域AgentReference::Entity投影为逻辑身份
    id: String--消息或turn ID
    workspace: WorkspaceReferenceDto--所属Workspace
    agent: String--稳定Agent逻辑ID或展示名称
    message: margatroid_types::Message--可展示消息正文
    from_domain(...) -> Self
        转换展示消息：由API应用层补充Workspace和Agent逻辑身份

AgentFailureDto：Agent失败展示DTO，公开结构体--把领域失败投影为可展示文本
    id: String--轮次或请求ID
    workspace: WorkspaceReferenceDto--所属Workspace
    agent: String--Agent稳定逻辑ID
    kind: String--失败来源
    message: String--有界错误描述

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
```

私有：
```text
WorkspaceAgentDefinitionDto：单Agent定义DTO，私有结构体--保存镜像、资源和Memory路径文本
ResourceRefDto：资源引用DTO，私有结构体--保存provider和scope/name文本
LogFieldDto：日志字段DTO，私有结构体--保存字段名和值
AgentHistoryDto：Agent历史DTO，私有结构体--保存Workspace引用、Agent ID和HistoryMessageDto列表
HistoryMessageDto：历史消息DTO，私有结构体--保存序号、turn ID、Message、资源标记和时间
```

## 逻辑

```text
统一入站信封：
    前端发送 { type, id, message }
        -> ServerPlugin::WebSocketMessageReceived
        -> ApiPlugin反序列化ClientRequest
        -> message DTO调用into_domain
        -> ApiPlugin直接发送领域事件

领域事件：
    workspace.start -> StartWorkspace
    agent.message -> RouteAgentMessage
    connection.register -> RegisterConnection

Agent消息路由：
    RouteAgentMessage携带WorkspaceReference和Agent逻辑名称
        -> WorkspacePlugin查询Workspace
        -> 生成稳定Agent ID或使用已有ID
        -> 发送AgentMessage { agent: AgentReference::Id }
        -> AgentPlugin解析AgentIdentity并使用Entity处理

出站：
    领域事件或状态
        -> API应用层构造ServerEvent
        -> ApiPlugin序列化并发送WebSocket文本
```

## 边界

```text
Protocol负责：
    定义跨进程JSON形状
    定义所有XxxDto
    实现DTO到共享领域值或领域命令的数据转换
    保证协议不暴露ECS Entity

Protocol不负责：
    查询Workspace或Agent
    生成Agent Entity
    决定manager
    执行消息意图、工具或推理

ApiPlugin负责：
    接收WebSocketMessageReceived
    解包统一信封
    调用DTO转换方法
    直接发送领域事件
    序列化ServerEvent并发送

ApiPlugin不再负责：
    发送StartWorkspace、RouteAgentMessage或RegisterConnection领域事件
    查找Workspace或Agent Entity
    执行领域业务
```
