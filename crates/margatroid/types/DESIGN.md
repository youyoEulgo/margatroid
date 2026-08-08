# MargatroidTypes

## 类型

公开：
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

ResourceName：资源逻辑名称，公开结构体--跨资源Loader共享scope/name标识
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

ResourceRefError：统一资源引用错误，公开枚举--描述Provider ID格式错误
    EmptyProvider
    InvalidProvider
    impl fmt::Display for ResourceRefError
        Display：公开trait实现，输出不包含原始输入的稳定错误描述
    impl std::error::Error for ResourceRefError
        Error：公开trait实现

ResourceRef：统一可调用资源引用，公开结构体--用同一种身份表示普通Tool、Skill、Workflow和未来工具资源
    provider: String--提供资源的工具定义Plugin稳定ID，例如tool、skill或workflow
    name: ResourceName--Provider内部的逻辑资源名称
    new(provider: impl Into<String>, name: ResourceName) -> Result<Self, ResourceRefError>
        构造引用：公开关联函数，验证provider非空，且只包含ASCII小写字母、数字、下划线和连字符
    provider(&self) -> &str
        取得Provider：公开方法，仅用于路由工具定义提供方
    name(&self) -> &ResourceName
        取得名称：公开方法
    impl Clone + Ord + Eq for ResourceRef
        值语义：公开trait实现，可直接作为可见性集合键

AgentImageReferenceError：AgentImage引用错误，公开枚举--描述scope/name:tag格式错误
    InvalidName
    InvalidTag
    impl fmt::Display for AgentImageReferenceError
        Display：公开trait实现，输出不包含原始输入的稳定错误描述
    impl std::error::Error for AgentImageReferenceError
        Error：公开trait实现

AgentImageReference：AgentImage引用，公开结构体--跨Loader和Workspace共享的规范化scope/name:tag标识
    resource: ResourceName--镜像scope/name，私有
    tag: String--镜像版本标签，私有
    new(value: impl Into<String>) -> Result<Self, AgentImageReferenceError>
        构造引用：公开关联函数，解析并验证scope/name:tag文本
        行为：
            tag省略时使用latest
            scope/name必须满足ResourceName规则
            tag长度为1到128 UTF-8字节
            tag只允许ASCII字母、数字、下划线、点和连字符
            tag首字符不能是点或连字符
            成功时保存ResourceName和规范化tag
    resource(&self) -> &ResourceName
        取得资源名：公开方法，返回scope/name
    scope(&self) -> &str
        取得作用域：公开方法，返回resource.scope
    name(&self) -> &str
        取得名称：公开方法，返回resource.name
    tag(&self) -> &str
        取得标签：公开方法，返回tag
    impl fmt::Display for AgentImageReference
        Display：公开trait实现，始终输出scope/name:tag

WorkspaceAgentDefinition：Workspace中的Agent静态定义，公开结构体--Compose交给WorkspacePlugin的单实例收集输入
    name: String--Workspace内唯一的Agent逻辑名称
    image: AgentImageReference--启动来源镜像
    resources: Vec<ResourceRef>--在镜像默认值上额外启用的可调用资源，统一表示Tool、Skill或Workflow
    disable_resources: Vec<ResourceRef>--最终禁用的资源，优先于镜像默认和额外项
    memory_path: Option<PathBuf>--可选Memory SQLite覆盖路径；空时由WorkspacePlugin生成默认路径
    impl Clone for WorkspaceAgentDefinition
        Clone：公开trait实现
    限制：结构体不提供业务方法；Compose负责构造，WorkspacePlugin负责运行时复核与执行

WorkspaceDefinition：编译后的Workspace静态定义，公开结构体--不包含YAML语法和运行时Entity
    name: String--项目内Workspace逻辑名称
    project_root: PathBuf--Compose根据配置文件位置确定的绝对项目根
    manager: String--默认入口Agent逻辑名称
    agents: Vec<WorkspaceAgentDefinition>--需要创建的全部Agent定义，保持配置顺序
    impl Clone for WorkspaceDefinition
        Clone：公开trait实现
    限制：结构体不提供业务方法；它是进程内业务输入，不是CLI/daemon网络DTO

ToolCall：统一工具调用，公开结构体--保存前端指定或Provider返回且后续工具执行必须原样关联的调用
    id: String--调用来源生成的工具调用ID
    name: String--模型可见工具名称
    arguments: String--完整参数JSON文本
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
    }
    Assistant {
        content: Option<String>--Assistant文本，只有工具调用时允许为空
        tool_calls: Vec<ToolCall>--完整工具调用列表，可以为空
    }
    Tool {
        tool_call_id: String--对应Assistant ToolCall::id
        content: String--工具成功输出或稳定错误文本
    }
    impl Clone for Message
        Clone：公开trait实现
    impl PartialEq + Eq for Message
        值语义：公开trait实现
    impl Serialize + Deserialize for Message
        序列化：公开trait实现，MemoryPlugin使用结构化JSON持久化动态消息

MessageIntent：统一消息意图，公开枚举--由产生消息的可信来源直接指定AgentPlugin后续动作
    UserWithToolCalls { tool_calls: Vec<ToolCall> }--用户入口赋予，先执行前端随本次用户消息指定的实际工具调用，不立即发起推理
    UserWithoutToolCalls--用户入口赋予，提交用户消息后直接发起推理
    DispatchToolCalls--InferencePlugin赋予，提交Assistant消息后发送工具调用
    ResolveToolCall--ToolPlugin赋予，提交Tool消息后使用更新完成的上下文继续推理
    CompleteTurn--InferencePlugin赋予，提交最终Assistant消息后结束本轮
    impl Clone + PartialEq + Eq for MessageIntent
        值语义：公开trait实现

AgentReference：Agent实例引用，公开枚举--允许跨边界使用稳定Agent ID，也允许运行时Plugin使用Entity
    Entity(Entity)--已经解析的Agent ECS Entity，仅供后端内部事件使用
    Id(String)--Agent稳定逻辑ID，例如demo.coder0，供API入口和跨Plugin消息使用
    impl Clone + PartialEq + Eq for AgentReference
        值语义：公开trait实现

AgentMessage：统一Agent消息事件，公开结构体--Margatroid内部所有成功消息来源交给AgentPlugin的唯一格式
    id: String--完整用户交互轮次ID
    agent: AgentReference--消息所属Agent，可为外部稳定ID或已解析Entity
    message: Message--需要写入Agent动态上下文的统一消息
    intent: MessageIntent--消息来源已经决定的后续处理意图
    impl Event for AgentMessage
        Event：公开trait实现
    impl Clone for AgentMessage
        Clone：公开trait实现
    限制：message只能是User、Assistant或Tool，不能是System；Id只允许作为进入AgentPlugin前的引用；结构体不提供业务方法；来源Plugin负责构造，AgentPlugin负责解析引用、记入消息和执行intent

AgentContextMessagesUpdated：Agent上下文更新事件，公开结构体--AgentContext修改完成后通知MemoryPlugin同步实时消息表
    agent: Entity--消息所属AgentInstance Entity
    messages: Vec<Message>--修改完成后的完整动态消息快照，不包含System消息
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

MessageResource：消息使用的资源引用，公开类型别名--等于ResourceRef，只记录统一身份，不携带资源正文

AgentResourcesUsed：Agent轮次资源使用事件，公开结构体--由实际解析资源的来源报告给MemoryPlugin
    id: String--使用资源的完整交互轮次ID
    agent: Entity--使用资源的AgentInstance Entity
    resources: Vec<MessageResource>--本次新增使用的资源引用，不包含正文
    impl Event for AgentResourcesUsed
        Event：公开trait实现
    impl Clone for AgentResourcesUsed
        Clone：公开trait实现
```

## 函数

私有：
```text
validate_part(part: &str) -> Result<(), ResourceNameError>
    验证名称段：私有函数，拒绝空值、.、..、控制字符和反斜杠

validate_tag(tag: &str) -> Result<(), AgentImageReferenceError>
    验证标签：私有函数，执行AgentImageReference的长度、字符和首字符规则

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
        -> WorkspacePlugin、SkillPlugin和WorkflowPlugin共享同一值类型
        -> WorkspacePlugin用它保存可见名称，资源Plugin用它拼接受控资源根

构造AgentImage引用：
    AgentImageReference::new(value)
        -> 按第一个冒号拆分scope/name与可选tag
        -> 没有tag时使用latest
        -> ResourceName::new(scope/name)
        -> validate_tag(tag)
        -> 保存resource与tag

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
        -> 构造Message::User
        -> 前端没有指定工具调用时赋予UserWithoutToolCalls
        -> 前端指定Skill、Workflow或其他工具调用时赋予UserWithToolCalls { tool_calls }
        -> 发送AgentMessage
    AgentPlugin收到UserWithoutToolCalls
        -> 直接使用当前完整上下文发起推理
    AgentPlugin收到UserWithToolCalls
        -> 先发送前端指定的tool_calls
        -> 不立即发起推理
    InferencePlugin完成推理
        -> 构造Message::Assistant
        -> 根据tool_calls赋予DispatchToolCalls或CompleteTurn
        -> 发送AgentMessage
    ToolPlugin完成工具调用
        -> 构造Message::Tool
        -> 赋予ResolveToolCall
        -> 发送AgentMessage
    AgentPlugin收到ResolveToolCall
        -> 将Tool消息追加到上下文后发起推理
    AgentPlugin
        -> 只消费统一AgentMessage
        -> 记入message并根据intent更新AgentStatus
        -> 不重新判断消息来源或生成意图
        -> 每次发起推理都按AgentDynamicVisibility构造tools；用户意图不控制工具定义是否进入请求

失败通道：
    推理失败不能伪装成Message
        -> InferencePlugin发送AgentFailure
        -> 后续处理契约暂不定义

资源使用元数据：
    SkillPlugin、WorkflowPlugin或其他资源来源解析实际内容
        -> 内容只用于临时模型请求
        -> 直接报告本次Tool对应的ResourceRef
        -> 发送AgentResourcesUsed { id, agent, resources }
    MemoryPlugin只保存引用，不把资源正文混入Message
```

## 持有关系

```text
ResourceName
├── scope: String
└── name: String

AgentImageReference
├── resource: ResourceName
└── tag: String

WorkspaceDefinition
├── name
├── project_root
├── manager
└── agents: Vec<WorkspaceAgentDefinition>
    ├── name
    ├── image: AgentImageReference
    ├── resources: Vec<ResourceRef>
    ├── disable_resources: Vec<ResourceRef>
    └── memory_path

Message
├── System / User
├── Assistant
│   └── Vec<ToolCall>
└── Tool

AgentMessage
├── id
├── agent: Entity
├── message: Message
└── intent: MessageIntent

AgentFailure
├── id
├── agent: Entity
├── kind: AgentFailureKind
└── message

AgentResourcesUsed
├── id
├── agent: Entity
└── resources: Vec<MessageResource = ResourceRef>
    ├── provider
    └── name: ResourceName
```
