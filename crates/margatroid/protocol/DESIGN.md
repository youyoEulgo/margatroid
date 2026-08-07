# MargatroidProtocol

## 类型

公开：
~~~text
ClientRequest：CLI发给daemon的请求，公开枚举--当前只承载Workspace启动请求
    WorkspaceStart {
        id: String--调用方生成的请求ID
        definition: WorkspaceDefinitionDto--编译后的Workspace静态定义
    }
    impl Serialize + Deserialize for ClientRequest
        Serialize + Deserialize：公开trait实现，使用type字段区分workspace.start
    start_workspace(id: impl Into<String>, definition: &WorkspaceDefinition) -> Self
        构造启动请求：公开关联函数，将领域定义转换为跨进程DTO

ServerEvent：daemon发给CLI的事件，公开枚举--当前只承载日志事件
    Log { record: LogRecordDto }--后端日志记录
    impl Serialize + Deserialize for ServerEvent
        Serialize + Deserialize：公开trait实现，使用type字段区分log

LogRecordDto：日志跨进程记录，公开结构体--保存CLI展示所需的结构化日志
    timestamp_millis: u64--日志时间
    level: String--日志级别
    target: String--日志目标
    message: String--日志正文
    fields: Vec<LogFieldDto>--结构化字段
    spans: Vec<String>--Tracing span名称
    impl Serialize + Deserialize for LogRecordDto
        Serialize + Deserialize：公开trait实现

LogFieldDto：日志字段，公开结构体--保存结构化字段名称和值
    name: String--字段名
    value: String--字段文本
    impl Serialize + Deserialize for LogFieldDto
        Serialize + Deserialize：公开trait实现

WorkspaceDefinitionDto：Workspace跨进程定义，公开结构体--保存不含ECS Entity的可序列化定义
    name: String--Workspace逻辑名称
    project_root: String--绝对项目根路径文本
    manager: String--默认入口Agent名称
    agents: Vec<WorkspaceAgentDefinitionDto>--Agent定义，保持Compose顺序
    from_definition(definition: &WorkspaceDefinition) -> Self
        转换定义：公开关联函数，将领域定义转换为DTO
    into_definition(self) -> Result<WorkspaceDefinition, ProtocolError>
        恢复定义：公开方法，解析镜像和资源引用并构造领域定义；WorkspacePlugin继续做运行时复核
    impl Serialize + Deserialize for WorkspaceDefinitionDto
        Serialize + Deserialize：公开trait实现，字段使用JSON稳定文本格式

WorkspaceAgentDefinitionDto：Agent跨进程定义，公开结构体--保存单个Agent的可序列化配置
    name: String--Agent逻辑名称
    image: String--scope/name[:tag]镜像引用文本
    resources: Vec<ResourceRefDto>--额外可见资源
    disable_resources: Vec<ResourceRefDto>--禁用资源
    memory_path: Option<String>--可选SQLite路径文本
    impl Serialize + Deserialize for WorkspaceAgentDefinitionDto
        Serialize + Deserialize：公开trait实现

ResourceRefDto：资源跨进程引用，公开结构体--保存Provider和scope/name文本
    provider: String--工具定义Plugin ID
    name: String--资源逻辑名称
    impl Serialize + Deserialize for ResourceRefDto
        Serialize + Deserialize：公开trait实现

ProtocolErrorKind：协议定义恢复错误分类，公开枚举
    InvalidImageReference
    InvalidResourceReference
    impl Clone + Copy + PartialEq + Eq for ProtocolErrorKind
        值语义：公开trait实现

ProtocolError：协议定义恢复错误，公开结构体--保存稳定分类和有界描述
    kind: ProtocolErrorKind--错误分类，私有
    message: String--错误描述，私有
    kind(&self) -> ProtocolErrorKind
        取得分类：公开方法
    message(&self) -> &str
        取得描述：公开方法
    impl Clone + PartialEq + Eq for ProtocolError
        值语义：公开trait实现
    impl fmt::Display for ProtocolError
        Display：公开trait实现
    impl std::error::Error for ProtocolError
        Error：公开trait实现
~~~

## 函数

私有：
~~~text
WorkspaceAgentDefinitionDto::from_definition(definition: &WorkspaceAgentDefinition) -> Self
    转换Agent：私有方法，将镜像、资源和Memory路径转换为字符串

WorkspaceAgentDefinitionDto::into_definition(self) -> Result<WorkspaceAgentDefinition, ProtocolError>
    恢复Agent：私有方法，解析镜像和资源字符串

ResourceRefDto::from_resource(resource: &ResourceRef) -> Self
    转换资源：私有方法，将ResourceRef转换为provider/name字段

ResourceRefDto::into_resource(self) -> Result<ResourceRef, ProtocolError>
    恢复资源：私有方法，调用ResourceName::new和ResourceRef::new
~~~

## 逻辑

~~~text
CLI编译Workspace文件
    -> Compose::compile返回WorkspaceDefinition
    -> ClientRequest::start_workspace生成WorkspaceDefinitionDto
    -> serde_json序列化为{"type":"workspace.start", ...}
    -> 通过CLI持有的WebSocket发送给daemon

daemon收到workspace.start
    -> serde_json反序列化ClientRequest
    -> WorkspaceDefinitionDto::into_definition恢复领域值
    -> WorkspacePlugin接收StartWorkspace
    -> WorkspacePlugin执行名称、路径和运行时资源复核

边界：
    Protocol不创建Entity、不连接WebSocket、不读取配置文件、不处理LLM消息
    Protocol只定义跨进程数据形状；传输由CLI和ServerPlugin完成
~~~

## 持有关系

~~~text
ClientRequest
└── WorkspaceDefinitionDto
    └── Vec<WorkspaceAgentDefinitionDto>
        ├── Vec<ResourceRefDto>
        ├── Vec<ResourceRefDto>
        └── memory_path
~~~
