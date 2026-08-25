# MargatroidTypes

## 跨Plugin值类型归属

```text
Agent以及其内嵌的AgentInfo、AgentCreationState、AgentLuaState、AgentMcl、AgentResourceMap、AgentMemoryHandle、AgentMemoryStore、AgentMemoryStoreError、AgentInferenceState、AgentToolState、AgentTurnState和TokenUsageState均定义在agent_plugin，对应agent_plugin/DESIGN.md的lib和types板块。
AgentPlugin负责创建Entity并分别挂载ResourceId和唯一Agent组件；Agent组件的存在本身表明该Entity是Agent。其他领域Plugin通过agent_plugin公开的Agent组件读取或修改自己负责的数据，不再为同一Agent挂载第二份状态Component。
MCL的纯值类型MclMessage、MclRealtimeSource、Block、BlockAssembly、RefMerge、RefBlock和RefBlockAssembly以及LuaVmId定义在types crate，由对应Plugin重新导出；它们不持有Agent生命周期或领域入口。
ResourceId及其World查询扩展实际定义在ResourceIdPlugin；types crate中的共享结构只引用ResourceId，不拥有其实现。
types crate只定义数据结构、局部数据方法和AgentMemoryStore这类依赖反转接口，不包含System、事件路由或数据库、推理、工具、Lua VM的具体实现。
```

### 规范归属与依赖方向

```text
resource_id_plugin是身份基础crate，只依赖core_plugin；ResourceId、ResourceIdError、ResourceIdLookupError和WorldResourceIdExt只在该crate定义。
types依赖resource_id_plugin并持有跨领域纯值：Block、RefBlock、MclMessage、MclRealtimeSource、LuaVmId、AgentError、ToolError和StopReason。
agent_plugin拥有唯一Agent聚合根及其内嵌状态；mcl_plugin、memory_plugin、tool_plugin和inference_plugin依赖agent_plugin访问该聚合根，并分别实现自己的System、Provider和领域事件。
types不得依赖任何业务Plugin；Agent的业务聚合状态统一由Agent组件持有，其他Plugin不得为同一个Agent重复挂载第二份资源映射、工具状态或生命周期状态Component。
AgentMcl的机械Block方法统一返回AgentError；各领域Plugin在边界处把AgentError转换为自己的错误类型。
AgentResourceMap的存储类型属于agent_plugin，ToolPlugin独占其注册、查询和pending调用的业务入口。
```

## 类型

### 统一资源身份

```text
ResourceId和ResourceIdError由ResourceIdPlugin公开，其他插件直接依赖ResourceIdPlugin使用
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
    message: Message--当前只接受纯User消息；手动工具调用使用独立命令或伪造Assistant工具调用
    impl Event for RouteAgentMessage

RouteAgentTurnAbort：逻辑Agent轮次中止命令，公开事件--由DTO层产生并交给WorkspacePlugin解析Entity
    id: String--API请求ID
    workspace: WorkspaceReference
    agent: Option<ResourceId>--空时路由到Workspace manager
    impl Event for RouteAgentTurnAbort
        Event：公开trait实现

AgentVisibilityRouteAction：Agent默认资源可见性操作，公开枚举
    Inject
    Remove

RouteAgentVisibility：逻辑Agent默认资源可见性路由命令，公开事件--由DTO层产生并交给WorkspacePlugin解析Entity
    id: String--请求ID
    workspace: WorkspaceReference--目标Workspace逻辑引用
    agent: Option<ResourceId>--目标Agent完整资源ID，None表示manager
    resource_id: ResourceId--待开关的完整资源ID
    action: AgentVisibilityRouteAction
    impl Event for RouteAgentVisibility

RouteAgentWorkflowAttach：逻辑Workflow挂载命令，公开事件--由DTO层产生并交给WorkspacePlugin解析Entity与资源根
    id: String--请求ID，同时作为新Workflow实例ID
    workspace: WorkspaceReference--目标Workspace逻辑引用
    agent: Option<ResourceId>--目标Agent完整资源ID，None表示manager
    resource_id: ResourceId--待挂载的mcl资源ID
    impl Event for RouteAgentWorkflowAttach

RouteAgentWorkflowDetach：逻辑Workflow卸载命令，公开事件--由DTO层产生并交给WorkspacePlugin解析Entity
    id: String--请求ID
    workspace: WorkspaceReference--目标Workspace逻辑引用
    agent: Option<ResourceId>--目标Agent完整资源ID，None表示manager
    instance_id: String--挂载时生成的Workflow实例ID
    impl Event for RouteAgentWorkflowDetach

ToolCall：统一工具调用，公开结构体--保存前端指定或Provider返回且后续工具执行必须原样关联的调用
    id: String--调用来源生成的工具调用ID
    tool_name: String--内部值为AgentResourceMap内唯一的resource_name；Provider名称由InferencePlugin临时转换
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
    }
    Assistant {
        reasoning: Option<String>--Provider公开的完整思考内容，可以为空；与正文分别保存
        content: Option<String>--Assistant文本，只有工具调用时允许为空
        tool_calls: Vec<ToolCall>--完整工具调用列表，可以为空
    }
    Tool {
        resource_id: ResourceId--本次调用对应的具体资源ID
        tool_call_id: String--对应Assistant ToolCall::id
        content: String--工具成功输出或稳定错误文本
    }
    Error {
        message: String--Agent完成创建后的轮次级稳定错误文本；由mcl_plugin从AgentFailure转换，Base Lua只写入历史
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
    usage: Option<TokenUsage>--仅InferencePlugin产生的Assistant响应携带本轮Token用量，其他来源固定为空
    impl Event for AgentMessage
        Event：公开trait实现
    impl Clone for AgentMessage
        Clone：公开trait实现
    限制：message可以是User、Assistant、Tool或Error，不能是System；结构体不提供业务方法；AgentPlugin只投递，Base Lua根据start返回的Message结构决定后续Effect

AgentFailureKind：Agent执行失败来源，公开枚举--标识无法表示成Message的轮次级失败
    Agent--AgentPlugin在消息分支、上下文或工具定义准备失败时产生
    Inference--InferencePlugin在准备或执行推理失败时产生
    Tool--ToolPlugin在调用无法路由且不能产生合法Tool消息时产生
    impl Clone + Copy + PartialEq + Eq for AgentFailureKind
        值语义：公开trait实现

AgentFailure：兼容性失败事件，公开结构体--仅供非Agent领域向观察者报告无法转换为AgentControlReply的失败；AgentPlugin自身不发布该事件
    id: String--原完整交互轮次ID
    agent: Entity--失败所属AgentInstance Entity
    kind: AgentFailureKind--失败来源分类
    message: String--来源Plugin生成的不含请求正文或密钥的有界稳定描述
    impl Event for AgentFailure
        Event：公开trait实现
    impl Clone for AgentFailure
        Clone：公开trait实现

AgentHistoryMessageWriteRequested：Agent历史消息写入请求，公开结构体--MclPlugin的history_append Effect通过事件通道交给MemoryPlugin
    id: String--完整交互轮次ID
    agent: Entity--消息所属AgentInstance Entity
    message: Message--需要写入历史的消息，Skill响应已由AgentPlugin替换为加载标记
    tool_schema: Vec<ToolDefinition>--产生Assistant的该次推理实际ToolSpec；User和Tool固定为空
    usage: Option<TokenUsage>--Assistant响应的本轮Token用量；User和Tool固定为空
    impl Event for AgentHistoryMessageWriteRequested
        Event：公开trait实现
    impl Clone for AgentHistoryMessageWriteRequested
        Clone：公开trait实现

AgentError：Agent共享错误，公开结构体--跨Plugin传递Agent数据和生命周期失败
    kind: AgentErrorKind--稳定有限分类
    message: String--不包含消息正文、参数、密钥或绝对路径的有界描述
    impl Clone + PartialEq + Eq for AgentError
    impl fmt::Display + std::error::Error for AgentError

AgentErrorKind：Agent共享错误分类，公开枚举
    InvalidRequest
    AgentMissing
    DuplicateAgent
    ResourceMissing
    BlockMissing
    InnerMissing
    TypeMismatch
    LuaRuntime
    Mcl
    Import
    Inference
    Tool
    Memory
    Stopped

AgentInferencePending：推理飞行事务，公开结构体--只保存取消和关联所需的最小定位
    id: String
    tool_schema: Vec<ToolDefinition>

AgentToolPending：工具飞行事务，公开结构体--只保存迟到响应匹配所需的定位
    turn_id: String
    tool_call_id: String
    resource_id: ResourceId
    tool_id: ResourceId

AgentToolEnvironment：工具运行环境，公开结构体--由Workspace构造并由具体工具Provider读取
    project_root: PathBuf
    image_root: PathBuf
    impl Clone

StopReason：Provider无关推理停止原因，公开枚举
    Completed
    ToolCalls
    Length
    Cancelled
    Error
    impl Clone + Copy + PartialEq + Eq

TokenUsage：单次模型响应Token用量，公开纯数据结构
    input_tokens: u64--本次请求输入Token数
    output_tokens: u64--本次响应输出Token数
    cache_hit_tokens: u64--本次输入中命中Provider缓存的Token数；Provider未提供该字段时为0
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
        -> WorkspacePlugin、ToolPlugin和具体资源执行器共享同一值类型
        -> WorkspacePlugin用它保存可见资源，ToolPlugin用type选择隐藏执行器

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
        -> 构造Message::User { content }
        -> 发送AgentMessage
    AgentPlugin收到AgentMessage
        -> 只投递给Agent.lua.vm_id对应的长期Lua VM
    InferencePlugin完成推理
        -> Provider Adapter保留reasoning与tool_name并构造Message::Assistant
        -> 发送AgentMessage
    ToolPlugin完成工具调用
        -> 从Agent.tools.pending恢复resource_id并构造Message::Tool
        -> 发送AgentMessage
    AgentPlugin
        -> 只消费AgentControl和AgentMessage
        -> AgentControl按控制类型路由到领域请求
        -> AgentMessage不解析内容，只投递给长期Lua VM
        -> Base Lua通过MCL、Inference和Tool领域组织完整控制循环

失败通道：
    Agent创建成功后的轮次级失败
        -> InferencePlugin发送AgentFailure
        -> mcl_plugin转换为AgentMessage(Error)投递给Agent
        -> Base Lua收到Error只执行history_append写入历史
        -> 前端从历史渲染错误消息

记忆事件：
    Base Lua通过MCL HistoryAppend Effect
        -> 发送AgentHistoryMessageWriteRequested
        -> MCL conversation保存完整Tool正文
    MCL realtime_source声明或其依赖字段修改
        -> MemoryPlugin定义的AgentRealtimeContextWriteRequested携带完整MclMessage快照
    MemoryPlugin通过事件执行持久化，并从Agent.memory取得目标存储句柄；不维护第二份Agent内存状态
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
├── message: Message
└── tool_schema: Vec<ToolDefinition>
```
