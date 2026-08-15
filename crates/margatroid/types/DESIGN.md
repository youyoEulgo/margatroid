# MargatroidTypes

## 类型

### 统一资源身份

```text
ResourceId：统一资源ID，公开结构体--所有可寻址资源共享的稳定身份
    resource_type: String--资源类型，私有
    scope: String--资源命名空间，私有
    name: String--命名空间内名称，私有
    tag: String--版本或实例标签，私有；省略时规范化为latest
    parse(value: impl AsRef<str>) -> Result<Self, ResourceIdError>
        解析ID：公开关联函数，解析type:scope/name[:tag]并补齐latest
    new(resource_type: impl Into<String>, scope: impl Into<String>, name: impl Into<String>, tag: Option<impl Into<String>>) -> Result<Self, ResourceIdError>
        构造ID：公开关联函数，验证字段并在tag为空时补齐latest
    resource_type(&self) -> &str
        取得资源类型：公开方法，用于选择资源路由
    scope(&self) -> &str
        取得作用域：公开方法
    name(&self) -> &str
        取得名称：公开方法
    tag(&self) -> &str
        取得标签：公开方法
    impl fmt::Display for ResourceId
        Display：公开trait实现，始终输出type:scope/name:tag
    impl FromStr for ResourceId
        FromStr：公开trait实现，行为与parse一致
    impl Clone + Ord + Eq + Hash for ResourceId
        值语义：公开trait实现，四个字段共同参与比较和哈希
    impl Serialize + Deserialize for ResourceId
        序列化：公开trait实现，使用规范化完整ID字符串

ResourceIdError：统一资源ID错误，公开枚举--描述ResourceId解析和验证错误
    Empty
    InvalidType
    InvalidScope
    InvalidName
    InvalidTag
    InvalidFormat
    impl fmt::Display for ResourceIdError
        Display：公开trait实现，输出不包含原始输入的稳定错误描述
    impl std::error::Error for ResourceIdError
        Error：公开trait实现
```

```text
ResourceId是跨Plugin查找资源的统一地址，不等同于ECS Entity句柄
资源可以是Agent、Workspace、AgentImage、Skill、Workflow、Tool等领域对象
ResourceId的规范格式为type:scope/name:tag，省略tag时使用latest
所有跨Plugin、DTO、配置、日志、Memory和前端身份都使用规范化ResourceId
ResourceName、ResourceRef和AgentImageReference只在迁移兼容层存在，不能作为新的领域身份
```

迁移兼容公开类型：
```text
ResourceNameError：资源名称错误，公开枚举--描述scope/name逻辑名称的格式错误
    Empty
    InvalidScope
    InvalidName
    InvalidCharacter
    impl fmt::Display for ResourceNameError
        Display：公开trait实现，输出不包含原始输入的稳定错误描述
    impl std::error::Error for ResourceNameError
        Error：公开trait实现

ResourceName：旧资源逻辑名称，迁移兼容结构体--只用于读取旧scope/name输入
    scope: String--资源作用域，私有
    name: String--作用域内名称，私有
    new(value: impl Into<String>) -> Result<Self, ResourceNameError>
        构造名称：公开关联函数，解析并验证scope/name文本
        行为：
            value为空时返回Empty
            value必须恰好包含scope和name两个非空路径段
            scope和name不能是.或..
            scope和name不能包含控制字符或反斜杠
            成功时分别保存scope和name
    scope(&self) -> &str
        取得作用域：公开方法，返回scope
    name(&self) -> &str
        取得名称：公开方法，返回name
    impl fmt::Display for ResourceName
        Display：公开trait实现，输出scope/name

旧ResourceRef、ResourceName和AgentImageReference：迁移兼容类型--仅用于读取旧配置和旧协议
    约束：进入领域层后必须转换为ResourceId，不得继续作为Entity身份或可见性键

WorkspaceAgentDefinition：Workspace中的Agent静态定义，公开结构体--Compose交给WorkspacePlugin的单实例收集输入
    name: String--Workspace内唯一的Agent逻辑名称
    id: ResourceId--完整Agent实例ID，格式agent:<workspace>/<name>:<tag>
    image: ResourceId--type必须为image的启动来源镜像
    resources: Vec<ResourceId>--在镜像默认值上额外启用的资源
    disable_resources: Vec<ResourceId>--最终禁用的资源
    memory_path: Option<PathBuf>--可选Memory SQLite覆盖路径；空时由WorkspacePlugin生成默认路径
    impl Clone for WorkspaceAgentDefinition
        Clone：公开trait实现
    限制：结构体不提供业务方法；Compose负责构造，WorkspacePlugin负责运行时复核与执行

WorkspaceDefinition：编译后的Workspace静态定义，公开结构体--不包含YAML语法和运行时Entity
    id: ResourceId--完整Workspace资源ID，默认格式workspace:local/<name>:latest
    name: String--项目内Workspace逻辑名称
    project_root: PathBuf--Compose根据配置文件位置确定的绝对项目根
    manager: String--默认入口Agent逻辑名称
    agents: Vec<WorkspaceAgentDefinition>--需要创建的全部Agent定义，保持配置顺序
    impl Clone for WorkspaceDefinition
        Clone：公开trait实现
    限制：结构体不提供业务方法；它是进程内业务输入，不是CLI/daemon网络DTO

WorkspaceReference：Workspace逻辑引用，公开结构体--使用名称和项目根跨Plugin定位已启动Workspace
    id: ResourceId--type必须为workspace的完整Workspace资源ID
    name: String--Workspace名称
    project_root: PathBuf--规范化项目根
    impl Clone + PartialEq + Eq for WorkspaceReference
        值语义：公开trait实现

StartWorkspace：Workspace启动命令，公开事件--把请求ID和静态定义交给WorkspacePlugin
    id: String--客户端请求ID
    definition: WorkspaceDefinition--待启动静态定义
    impl Event for StartWorkspace
        Event：公开trait实现

RouteAgentMessage：逻辑Agent消息路由命令，公开事件--由DTO层产生并交给WorkspacePlugin解析Entity
    id: String--完整交互轮次ID
    workspace: WorkspaceReference--目标Workspace逻辑引用
    agent: Option<ResourceId>--目标Agent完整资源ID，None表示manager
    message: Message--当前只接受User消息，前端预选调用保存在Message::User.tool_calls
    impl Event for RouteAgentMessage
        Event：公开trait实现

AgentSkillRouteAction：Agent持久Skill操作，公开枚举
    Load
    Unload
    UnloadAll

RouteAgentSkill：逻辑Agent持久Skill路由命令，公开事件--由DTO层产生并交给WorkspacePlugin解析Entity
    id: String--请求ID
    workspace: WorkspaceReference--目标Workspace逻辑引用
    agent: Option<ResourceId>--目标Agent完整资源ID，None表示manager
    resource_id: Option<ResourceId>--Load与Unload必填，UnloadAll为空
    action: AgentSkillRouteAction
    impl Event for RouteAgentSkill

ToolCall：统一工具调用，公开结构体--保存前端指定或Provider返回且后续工具执行必须原样关联的调用
    id: String--调用来源生成的工具调用ID
    tool_name: String--所属AgentToolMap内唯一的模型工具名
    arguments: String--完整参数JSON对象文本，无参数时为"{}"
    impl Clone + PartialEq + Eq for ToolCall
        值语义：公开trait实现
    impl Serialize + Deserialize for ToolCall
        序列化：公开trait实现，供Message持久化

ToolDefinition：统一工具定义，公开结构体--描述一次推理允许模型调用的工具
    name: String--模型可见工具名称
    description: String--工具说明
    input_schema: serde_json::Value--JSON Schema参数定义
    impl Clone for ToolDefinition
        Clone：公开trait实现
    impl Serialize + Deserialize for ToolDefinition
        序列化：公开trait实现

Message：统一消息，公开枚举--所有Margatroid消息Plugin共享的静态对话格式
    System {
        content: String--系统提示词
    }
    User {
        content: String--用户内容
        tool_calls: Vec<ToolCall>--用户预先指定的工具调用，可以为空
    }
    Assistant {
        content: Option<String>--Assistant文本，只有工具调用时允许为空
        tool_calls: Vec<ToolCall>--完整工具调用列表，可以为空
    }
    Tool {
        resource_id: ResourceId--本次调用对应的具体资源ID
        tool_call_id: String--对应Assistant ToolCall::id
        content: String--工具成功输出或稳定错误文本
    }
    impl Clone for Message
        Clone：公开trait实现
    impl PartialEq + Eq for Message
        值语义：公开trait实现
    impl Serialize + Deserialize for Message
        序列化：公开trait实现，MemoryPlugin使用结构化JSON持久化动态消息

AgentMessage：统一Agent消息事件，公开结构体--Margatroid内部所有成功消息来源交给AgentPlugin的唯一格式
    id: String--完整用户交互轮次ID
    agent: Entity--消息所属Agent，进入事件队列前必须完成逻辑身份解析
    message: Message--需要写入Agent动态上下文的统一消息
    impl Event for AgentMessage
        Event：公开trait实现
    impl Clone for AgentMessage
        Clone：公开trait实现
    限制：message只能是User、Assistant或Tool，不能是System；结构体不提供业务方法；AgentPlugin根据Message结构决定后续动作

AgentContextMessagesUpdated：Agent上下文更新事件，公开结构体--AgentContext修改完成后通知MemoryPlugin同步实时消息表
    agent: Entity--消息所属AgentInstance Entity
    messages: Vec<Message>--修改完成后的完整动态消息快照，不包含System消息
    tool_context: Vec<Message>--当前轮临时工具上下文，包含普通Tool输出和Skill正文
    impl Event for AgentContextMessagesUpdated
        Event：公开trait实现
    impl Clone for AgentContextMessagesUpdated
        Clone：公开trait实现

AgentFailureKind：Agent执行失败来源，公开枚举--标识无法表示成Message的轮次级失败
    Agent--AgentPlugin在消息分支、上下文或工具定义准备失败时产生
    Inference--InferencePlugin在准备或执行推理失败时产生
    impl Clone + Copy + PartialEq + Eq for AgentFailureKind
        值语义：公开trait实现

AgentFailure：统一Agent失败事件，公开结构体--让来源Plugin终止轮次而不伪造Assistant或Tool消息
    id: String--原完整交互轮次ID
    agent: Entity--失败所属AgentInstance Entity
    kind: AgentFailureKind--失败来源分类
    message: String--来源Plugin生成的不含请求正文或密钥的有界稳定描述
    impl Event for AgentFailure
        Event：公开trait实现
    impl Clone for AgentFailure
        Clone：公开trait实现

AgentHistoryMessageWriteRequested：Agent历史消息写入请求，公开结构体--AgentPlugin通过事件通道交给MemoryPlugin
    id: String--完整交互轮次ID
    agent: Entity--消息所属AgentInstance Entity
    message: Message--需要写入历史的消息，Skill响应已由AgentPlugin替换为加载标记
    impl Event for AgentHistoryMessageWriteRequested
        Event：公开trait实现
    impl Clone for AgentHistoryMessageWriteRequested
        Clone：公开trait实现
```

## 函数

私有：
```text
validate_part(part: &str) -> Result<(), ResourceNameError>
    验证名称段：私有函数，拒绝空值、.、..、控制字符和反斜杠

validate_resource_type(resource_type: &str) -> Result<(), ResourceIdError>
    验证资源类型：私有函数，只接受非空的小写ASCII字母、数字、下划线和连字符

validate_resource_part(part: &str, error: ResourceIdError) -> Result<(), ResourceIdError>
    验证scope或name：私有函数，拒绝空值、.、..、控制字符、分隔符和冒号

validate_resource_tag(tag: &str) -> Result<(), ResourceIdError>
    验证资源tag：私有函数，检查长度、首字符和允许字符

is_tag_character(character: char) -> bool
    检查标签字符：私有函数，只接受ASCII字母、数字、下划线、点和连字符
```

## 逻辑

```text
构造：
    ResourceName::new(value)
        -> 按/拆分value
        -> 必须恰好得到scope和name
        -> 分别调用validate_part
        -> 保存两个名称段

作为资源键：
    AgentImageLoaderPlugin发现scope/name目录
        -> 构造ResourceName
        -> WorkspacePlugin、BuiltinToolPlugin和具体资源执行器共享同一值类型
        -> WorkspacePlugin用它保存可见资源，BuiltinToolPlugin用type选择隐藏执行器

构造统一资源ID：
    ResourceId::parse(value)
        -> 拆分type与scope/name[:tag]
        -> 没有tag时使用latest
        -> 验证type、scope、name和tag
        -> 保存四个字段

Workspace定义：
    Compose读取margatroid-workspace.yaml
        -> 解析并规范化项目路径和资源名称
        -> 构造WorkspaceDefinition与WorkspaceAgentDefinition
        -> 不创建Entity，不加载AgentImage或资源正文
    WorkspacePlugin接收WorkspaceDefinition
        -> 复核名称唯一性和项目根
        -> 从各业务Plugin收集运行时实例材料

统一消息：
    用户入口
        -> 构造Message::User { content, tool_calls }
        -> 发送AgentMessage
    AgentPlugin收到User
        -> tool_calls为空时直接发起推理
        -> tool_calls非空时先派发指定工具
    InferencePlugin完成推理
        -> Provider Adapter保留tool_name并构造Message::Assistant
        -> 发送AgentMessage
    ToolPlugin完成工具调用
        -> 从PendingToolCalls恢复resource_id并构造Message::Tool
        -> 发送AgentMessage
    AgentPlugin
        -> 只消费统一AgentMessage
        -> 根据Message变体及ToolCall列表维护上下文、当前turn和loading skills
        -> 把工具调用发送为ToolCallEvent；pending与批次完成由ToolPlugin管理
        -> 每次发起推理都按AgentDynamicVisibility从AgentToolMap构造ToolSpec；用户意图不控制工具定义是否进入请求

失败通道：
    推理失败不能伪装成Message
        -> InferencePlugin发送AgentFailure
        -> 后续处理契约暂不定义

记忆事件：
    AgentPlugin处理User、Assistant或Tool
        -> 发送AgentHistoryMessageWriteRequested
        -> 历史事件的content直接替换为Message::Tool.resource_id字符串
        -> 实时tool_context仍保存完整Tool正文
    AgentContext修改
        -> 发送同时包含messages和tool_context的AgentContextMessagesUpdated
    MemoryPlugin只消费事件，不读取AgentStatus或资源正文
```

## 持有关系

```text
ResourceName
├── scope: String
└── name: String

ResourceId
├── resource_type: String
├── scope: String
├── name: String
└── tag: String

WorkspaceDefinition
├── name
├── project_root
├── manager
└── agents: Vec<WorkspaceAgentDefinition>
    ├── name
    ├── id: ResourceId
    ├── image: ResourceId
    ├── resources: Vec<ResourceId>
    ├── disable_resources: Vec<ResourceId>
    └── memory_path

Message
├── System
├── User
│   └── Vec<ToolCall>
├── Assistant
│   └── Vec<ToolCall>
└── Tool
    ├── resource_id: ResourceId
    └── tool_call_id

AgentMessage
├── id
├── agent: Entity
└── message: Message

AgentFailure
├── id
├── agent: Entity
├── kind: AgentFailureKind
└── message

AgentHistoryMessageWriteRequested
├── id
├── agent: Entity
└── message: Message
```
