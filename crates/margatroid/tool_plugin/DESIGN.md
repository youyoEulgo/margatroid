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

# 板块格式

当前 tool_plugin 使用一个 lib.rs 加 handler 目录组织：

```text
src/
├── lib.rs              Plugin、类型、事件、系统、公开函数
└── handler/
    ├── mod.rs          模块声明
    ├── skill.rs        skill 资源注册与执行
    ├── hook.rs         hook 资源注册与 no-op 执行
    ├── lua.rs          lua 工具注册、准备与异步执行
    └── shell.rs        shell 工具注册、准备与异步执行
```

lib.rs 是 crate root；handler 目录只放四类内置工具的具体实现。

# lib

## 类型

公开：
```text
ToolErrorKind：工具错误分类，公开枚举
    AgentMissing
    ResourceMapMissing
    InvalidResource
    ResourceUnavailable
    RegistrationFailed
    ToolCallMissing
    InvalidDefinition
    ProviderMissing
    ResourceResolutionFailed
    AgentNotAlive
    ToolEnvironmentMissing
    ToolPluginMissing
    ToolAlreadyRegistered
    DuplicateResource
    InvalidRequest
    InvalidArguments
    ExecutionFailed

ToolError：工具错误，公开结构体--稳定分类和有界安全描述
    kind: ToolErrorKind--错误分类，私有
    message: String--有界描述，私有
    new(kind: ToolErrorKind, message: impl Into<String>) -> Self
        构造错误：公开关联函数，超长消息在字符边界截断
    kind(&self) -> ToolErrorKind
        读取分类：公开方法
    message(&self) -> &str
        读取描述：公开方法
    impl Clone + Debug + PartialEq + Eq for ToolError
    impl fmt::Display for ToolError
    impl std::error::Error for ToolError

AgentToolEnvironment：Agent 工具环境，公开组件
    project_root: Arc<PathBuf>--项目根目录，私有
    image_root: Arc<PathBuf>--镜像根目录，私有
    new(project_root: impl Into<PathBuf>, image_root: impl Into<PathBuf>) -> Self
        构造环境：公开关联函数
    project_root(&self) -> &Path
        读取项目根：公开方法
    image_root(&self) -> &Path
        读取镜像根：公开方法
    impl Component for AgentToolEnvironment

ToolTemplate：工具模板，公开结构体
    name: String--模型可见名称
    description: String--模型可见描述
    parameters: serde_json::Value--JSON Schema
    new(name, description, parameters) -> Result<Self, ToolError>
        构造模板：公开关联函数，校验描述非空且参数为对象

ResourceContent：资源内容，公开枚举
    Prompt { role: String, content: Arc<str> }

ResourceMapEntry：资源映射条目，公开结构体
    resource_id: ResourceId
    resource_name: String
    alias: Option<String>
    tool_id: Option<ResourceId>
    template: Option<ToolTemplate>
    content: Option<ResourceContent>

ToolRegisterRequest：工具注册请求，公开事件
    id: String--请求 ID
    agent: Entity--目标 Agent
    resource_id: ResourceId--资源 ID
    alias: Option<String>--Agent 内别名
    impl Event for ToolRegisterRequest

ToolRegisterResponse：工具注册响应，公开事件
    id: String--原请求 ID
    agent: Entity--目标 Agent
    resource_id: ResourceId--资源 ID
    alias: Option<String>--Agent 内别名
    result: Result<ResourceMapEntry, ToolError>--注册结果
    impl Event for ToolRegisterResponse

ToolCallRequest：工具调用请求，公开内部类型--不再作为 ECS 事件
    turn_id: String--轮次 ID
    agent: Entity--目标 Agent
    tool_id: ResourceId--隐藏执行器 ID
    resource_id: ResourceId--资源 ID
    tool_call_id: String--工具调用 ID
    arguments: String--参数 JSON 文本

CancelToolTurn：取消工具轮次，公开事件
    turn_id: String--轮次 ID
    agent: Entity--目标 Agent
    impl Event for CancelToolTurn

ToolPlugin：工具插件，公开结构体--统一安装内置工具根、注册路由和调用路由
    schedule: String--System 所属 Schedule，私有
    skill_root: Arc<PathBuf>--skill 根目录，私有
    hook_root: Arc<PathBuf>--hook 根目录，私有
    lua_root: Arc<PathBuf>--lua 工具根目录，私有
    shell_root: Arc<PathBuf>--shell 根目录，私有
    lua_limits: LuaExecutionLimits--Lua 执行限制，私有
    shell_limits: ShellExecutionLimits--Shell 执行限制，私有
    open(root: impl Into<PathBuf>) -> Result<Self, ToolError>
        打开插件：公开关联函数，要求 root 绝对且无父级跳转
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置 Schedule：公开构建方法
    impl Default for ToolPlugin
    impl Plugin for ToolPlugin
        build(self, app: &mut App)
            安装插件：要求 RuntimePlugin 和 AsyncRuntimePlugin 已安装
            插入 ToolPluginInstalled、四类 roots、lua/shell 限制、LuaHttpClient、PersistentShells
            依次挂载注册、调用路由、异步执行、任务结果、取消和清理 System

ToolPluginInstalled：工具插件安装标记，公开单元 Resource
    impl Resource for ToolPluginInstalled
```

公开函数：
```text
register_agent_resource(world: &mut World, agent: Entity, entry: ResourceMapEntry) -> Result<ResourceMapEntry, ToolError>
    注册 Agent 资源：公开函数，把可执行条目写入 Agent.resources

candidate_resource_entry(resource_id: ResourceId, alias: Option<String>, tool_id: ResourceId, template: ToolTemplate) -> Result<ResourceMapEntry, ToolError>
    构造候选资源条目：公开函数

resolve_agent_tool_definitions(world: &World, agent: Entity, resources: &[ResourceId]) -> Result<Vec<ToolDefinition>, ToolError>
    解析 Agent 工具定义：公开函数，要求每个可见资源恰好注册一次

validate_agent_tool_calls(world: &World, agent: Entity, calls: &[ToolCall]) -> Result<(), ToolError>
    校验 Agent 工具调用：公开函数，校验 tool_name 已注册
```

# system

## 函数

私有：
```text
tool_register_system(world: &mut World)
    注册路由 System：私有 System
    处理事件：ToolRegisterRequest
    行为：
        id 为空 -> InvalidRequest 响应
        skill/hook/shell/tool 类型交给对应 handler 系统
        tool:builtin/* 除 hook 外拒绝注册
        未知类型 -> ProviderMissing 响应

tool_call_route_system(world: &mut World)
    调用路由 System：私有 System
    处理事件：ToolCallEvent
    行为：
        校验事件、参数和 Agent 状态
        通过 Agent.resources.tool_by_name 解析出 ToolCallRequest
        写入 Agent.tools.pending
        skill/hook 同步执行并直接 finish_tool_call
        lua/shell 调用 prepare_lua_call / prepare_shell_call 启动异步执行
        准备失败时 finish_tool_call 发送错误 Tool 消息

tool_message_cleanup_system(world: &mut World)
    消息清理 System：私有 System
    处理事件：AgentMessage
    行为：对 Message::Tool，按 (agent, turn_id, tool_call_id) 清理 Agent.tools.pending

cancel_tool_turn_system(world: &mut World)
    取消工具轮次 System：私有 System
    处理事件：CancelToolTurn
    行为：保留 Agent.tools.pending 中不属于该轮次的条目

finish_tool_call(world: &mut World, request: ToolCallRequest, result: Result<String, ToolError>)
    完成工具调用：私有函数
    行为：从 pending 移除，发送 AgentMessage::Tool
```

# handler

```text
skill.rs  skill_register_system / execute_skill_call
hook.rs   hook_register_system / execute_hook_call
lua.rs    lua_tool_register_system / prepare_lua_call / execute_prepared_lua_tool / lua_task_result_system
shell.rs  shell_register_system / prepare_shell_call / execute_prepared_shell / shell_task_result_system
```

# 逻辑

```text
注册：
Base Lua IMPORT -> MclPlugin -> ToolRegisterRequest
    -> tool_register_system 或四类 handler 系统
    -> ToolRegisterResponse -> MclPlugin 完成 IMPORT

调用：
Base Lua EMIT EFFECT tool_call -> MclPlugin -> ToolCallEvent
    -> tool_call_route_system
        -> skill/hook: 同步执行 -> finish_tool_call -> AgentMessage::Tool
        -> lua/shell: prepare -> 异步执行 -> 异步 guard 发送 AgentMessage::Tool
    -> AgentPlugin 投递回 Base Lua
```

# 持有关系

```text
App
└── World
    ├── ToolPluginInstalled
    ├── SkillRoots / HookRoots / LuaRoots / ShellRoots
    ├── LuaExecutionLimits / LuaHttpClient
    ├── ShellExecutionLimits / PersistentShells
    └── Agent
        ├── resources: AgentResourceMap
        └── tools.pending: HashMap<(Entity, String, String), AgentToolPending>
