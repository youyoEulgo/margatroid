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

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        图书馆组件与 Plugin
# system     src/system.rs     System 函数
# handler    src/handler.rs    处理函数
# events     src/events.rs     事件类型
# types      src/types.rs      其余类型
# error      src/error.rs      Error 与公开错误分类
```

## lib

lib 只放图书馆组件和 Plugin。

图书馆组件是 Entity 必须挂载的领域组件，组件存在本身表明 Entity 的领域身份；例如：

```text
Agent Entity     必须挂载 Agent 组件 + ResourceId 组件
Workspace Entity 必须挂载 Workspace 组件 + ResourceId 组件
```

WorkspacePlugin 等 Plugin 结构体，以及 Workspace 图书馆组件和 WorldWorkspaceExt 扩展 trait，也放在 lib。

## system

system 放 System 函数。System 只负责读取本帧领域事件并克隆，然后调用 handler 中的对应处理函数；System 不展开业务逻辑。

## handler

handler 放处理函数。每个 System 读到的领域事件在 handler 中展开为完整业务逻辑。

## events

events 放事件类型。事件类型只包含字段和 `impl Event`，不实现业务逻辑。

## types

types 放除事件和错误外的其余类型：状态、注册表、回执等。

## error

error 放 Error 类型和公开错误分类。

# lib

## 类型

公开：
```text
Workspace：Workspace 图书馆组件，公开 Component--Workspace Entity 必须挂载 Workspace 和 ResourceId；组件存在本身表明 Entity 是 Workspace
    definition: Arc<WorkspaceDefinition>--Compose 编译出的静态定义，crate公开字段
    project_root: Arc<PathBuf>--项目根目录，crate公开字段
    manager_name: String--manager 名称，crate公开字段
    agents: BTreeMap<String, Entity>--已创建 Agent 名称到 Entity，crate公开字段
    states: BTreeMap<String, WorkspaceAgentState>--成员状态，crate公开字段
    definition(&self) -> &WorkspaceDefinition
        读取定义：公开方法
    project_root(&self) -> &Path
        读取项目根：公开方法
    manager(&self) -> Option<Entity>
        读取 manager Entity：公开方法
    agent(&self, name: &str) -> Option<Entity>
        按名称读取 Agent：公开方法
    state(&self, name: &str) -> Option<&WorkspaceAgentState>
        读取成员状态：公开方法
    states(&self) -> impl Iterator<Item = (&str, &WorkspaceAgentState)> + '_
        遍历成员状态：公开方法
    iter(&self) -> impl Iterator<Item = (&str, Entity)> + '_
        遍历成员 Entity：公开方法
    impl Component for Workspace

WorkspacePlugin：Workspace 运行编排插件，公开结构体--协调 Workspace 及 Agent 的创建、消息路由、MCL 命令和初始化收集
    agent_images_root: PathBuf--AgentImage 根目录，私有
    schedule: String--System 所属 Schedule，私有
    open(root: impl Into<PathBuf>) -> Result<Self, WorkspaceError>
        打开插件：公开关联函数，要求 root 非空
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置 Schedule：公开构建方法
    impl Default for WorkspacePlugin
        Default：公开 trait 实现，使用空 root 和 RuntimePlugin::UPDATE
    impl Plugin for WorkspacePlugin
        Plugin：公开 trait 实现
        build(self, app: &mut App)
            安装插件：要求 schedule 和 ResourceIdPlugin、AgentImageLoaderPlugin、AgentPlugin、ToolPlugin、MemoryPlugin、MclPlugin、LuaRuntimePlugin 已安装
            行为：
                插入 WorkspaceRegistry
                依次挂载 begin_workspace_command_system、route_agent_message_system、route_agent_turn_abort_system、route_mcl_command_system、collect_mcl_command_response_system、collect_agent_image_system、collect_agent_initialization_system
```

公开 trait：
```text
WorldWorkspaceExt：Workspace 世界扩展，公开 trait--为 World 提供 Workspace 查询和命令入口
    start_workspace(&self, id: impl Into<String>, definition: WorkspaceDefinition)
        启动 Workspace：发送 StartWorkspace 事件
    stop_workspace(&self, id: impl Into<String>, workspace: Entity)
        停止 Workspace：发送 StopWorkspace 事件
    workspace_by_id(&self, id: &ResourceId) -> Option<Entity>
        按资源 ID 查找 Workspace Entity
    workspaces(&self) -> Vec<Entity>
        读取存活 Workspace Entity 列表
    workspace_agent(&self, workspace: Entity, name: &str) -> Option<Entity>
        读取成员 Agent Entity
    workspace_manager(&self, workspace: Entity) -> Option<Entity>
        读取 manager Agent Entity
    workspace_of(&self, agent: Entity) -> Option<Entity>
        读取 Agent 所属 Workspace Entity
    impl WorldWorkspaceExt for World
        World：公开 trait 实现，按字段逐项实现
```

# system

## 函数

crate公开：
```text
begin_workspace_command_system(world: &mut World)
    命令入口 System：crate公开 System
    处理事件：StartWorkspace、StopWorkspace、StopWorkspaceByReference、ReloadWorkspace
    行为：
        克隆本帧全部命令并逐个处理
        StartWorkspace 调用 handler::create_workspace 并发送 StartWorkspaceResult
        StopWorkspace 调用 handler::stop_workspace_entity 并发送 StopWorkspaceResult
        StopWorkspaceByReference 先按资源 ID 定位并校验 project_root，再调用 stop_workspace_entity 并发送 StopWorkspaceByReferenceResult
        ReloadWorkspace 先 stop 旧 Workspace，再 create 新 Workspace，发送 ReloadWorkspaceResult

route_agent_message_system(world: &mut World)
    成员消息路由 System：crate公开 System
    处理事件：RouteAgentMessage
    行为：解析 Workspace 和可选 Agent ID；Agent 为空时使用 manager；成功时发送 AgentMessage

route_agent_turn_abort_system(world: &mut World)
    成员轮次中止路由 System：crate公开 System
    处理事件：RouteAgentTurnAbort
    行为：解析 Workspace 和可选 Agent ID；Agent 为空时使用 manager；成功时发送 AgentControl::AbortTurn

route_mcl_command_system(world: &mut World)
    MCL 命令路由 System：crate公开 System
    处理事件：RouteMclCommand
    行为：克隆本帧全部命令并逐个调用 handler::route_mcl_command；失败时直接通过命令回执返回错误字符串

collect_mcl_command_response_system(world: &mut World)
    MCL 命令响应收集 System：crate公开 System
    行为：逐个尝试接收 pending_mcl_commands 中的 oneshot 回执；Empty 时写回注册表，Closed 时返回错误

collect_agent_image_system(world: &mut World)
    镜像结果收集 System：crate公开 System
    处理事件：LoadAgentImageResult
    行为：
        按 event_id 从 pending_images 定位 Workspace 和成员名
        失败时标记成员 Failed 并记录带 workspace/agent 的日志
        成功时准备 AgentCreateRequest：读取镜像依赖、Base Driver、模型、Memory，并构造 image_sources
        image_sources 除依赖 source 字段外，还为每个 prompt 依赖读取镜像根目录 `<NAME 大写>.md` 内容
        发送 AgentCreateRequest 并把 oneshot 接收器写入 pending_agents
        逐个消费 pending_agents 回执；成功时把 Agent 写入 Workspace.agents 和 Ready 状态，失败时标记 Failed

collect_agent_initialization_system(world: &mut World)
    初始化收集 System：crate公开 System
    处理事件：AgentInitializationCompleted
    行为：按 Agent 所属 Workspace 和 ResourceId 解析成员名，写入 Workspace.agents 和 Ready 状态，并记录带 workspace/agent 的日志
```

# handler

## 函数

crate公开：
```text
stop_workspace_entity(world: &mut World, workspace: Entity) -> Result<(), WorkspaceError>
    停止 Workspace Entity：crate公开函数
    行为：
        Workspace 不存活返回 WorkspaceNotAlive
        遍历 Workspace.agents，逐个发送 AgentControl::Stop 并 despawn Agent
        从 WorkspaceRegistry 移除 workspaces、pending_images、pending_agents
        移除 WorkspaceModelRoutesRegistry 中的路由
        despawn Workspace

create_workspace(world: &mut World, definition: &WorkspaceDefinition) -> Result<Entity, WorkspaceError>
    创建 Workspace Entity：crate公开函数
    行为：
        ResourceId 已存在返回 DuplicateWorkspace
        创建 Entity 并插入 ResourceId 和 Workspace
        为每个 Agent 初始化 Creating 状态并发送 LoadAgentImage
        写入 WorkspaceRegistry.workspaces
        不等待 Agent 创建完成

route_mcl_command(world: &mut World, command: RouteMclCommand) -> Result<(), WorkspaceError>
    路由 MCL 命令：crate公开函数
    行为：
        解析 Workspace；命令带 agent_id 时按资源 ID 查找，否则使用 manager
        读取 Agent 的 ResourceId，创建 MclCommandId 和 oneshot 回执
        发送 MclCommandRequest，并把外部回执和 oneshot 接收器写入 pending_mcl_commands
```

# events

## 类型

公开：
```text
StartWorkspaceResult：Workspace 启动结果，公开事件
    id: String--原请求 ID
    result: Result<Entity, WorkspaceError>--成功时返回 Workspace Entity
    impl Event for StartWorkspaceResult

ReloadWorkspace：Workspace 重载请求，公开事件
    id: String--请求 ID
    workspace: Entity--当前 Workspace Entity
    definition: WorkspaceDefinition--重新编译的定义
    impl Event for ReloadWorkspace

ReloadWorkspaceResult：Workspace 重载结果，公开事件
    id: String--原请求 ID
    previous: Entity--被关闭的原 Workspace Entity
    result: Result<Entity, WorkspaceError>--新 Workspace Entity
    impl Event for ReloadWorkspaceResult

StopWorkspace：Workspace 停止请求，公开事件
    id: String--请求 ID
    workspace: Entity--目标 Workspace Entity
    impl Event for StopWorkspace

StopWorkspaceResult：Workspace 停止结果，公开事件
    id: String--原请求 ID
    workspace: Entity--原 Workspace Entity
    result: Result<(), WorkspaceError>--停止结果
    impl Event for StopWorkspaceResult

StopWorkspaceByReference：按逻辑引用停止 Workspace，公开事件
    id: String--请求 ID
    workspace: WorkspaceReference--Workspace 逻辑引用
    impl Event for StopWorkspaceByReference

StopWorkspaceByReferenceResult：按逻辑引用停止结果，公开事件
    id: String--原请求 ID
    workspace: WorkspaceReference--原逻辑引用
    result: Result<(), WorkspaceError>--停止结果
    impl Event for StopWorkspaceByReferenceResult
```

# types

## 类型

公开：
```text
WorkspaceAgentState：Workspace 成员状态，公开枚举
    Creating
    Ready { agent: Entity }
    Failed { error: WorkspaceError }
    impl Clone + Debug + PartialEq + Eq for WorkspaceAgentState

WorkspaceRegistry：WorkspacePlugin 注册表，公开 Resource
    agent_images_root: Arc<PathBuf>--AgentImage 根目录
    workspaces: Vec<Entity>--已创建 Workspace Entity
    pending_images: BTreeMap<String, (Entity, String)>--待处理镜像请求 ID 到 Workspace 和成员名
    pending_agents: BTreeMap<String, (Entity, String, oneshot::Receiver<Result<Entity, AgentError>>)>--待处理创建请求 ID 到 Workspace、成员名和回执接收器
    pending_mcl_commands: BTreeMap<String, (mpsc::Sender<Result<serde_json::Value, String>>, oneshot::Receiver<Result<MclCommandValue, MclError>>)>--待处理 MCL 命令 ID 到外部回执和内部回执接收器
    impl Default for WorkspaceRegistry
    impl Resource for WorkspaceRegistry
```

# error

## 类型

公开：
```text
WorkspaceErrorKind：Workspace 错误分类，公开枚举
    InvalidRequest
    InvalidDefinition
    InvalidProjectRoot
    InvalidAgentImagesRoot
    DuplicateWorkspace
    WorkspaceNotAlive
    WorkspaceNotReady
    WorkspaceMismatch
    AgentImageLoadFailed
    AgentImageComponentsMissing
    InferenceSetupFailed
    MemorySetupFailed
    ResourceSetupFailed
    AgentCreateFailed

WorkspaceError：Workspace 错误，公开结构体
    kind: WorkspaceErrorKind--错误分类，私有
    message: String--错误描述，私有
    new(kind: WorkspaceErrorKind, message: impl Into<String>) -> Self
        构造错误：公开关联函数
    kind(&self) -> WorkspaceErrorKind
        读取分类：公开方法
    message(&self) -> &str
        读取描述：公开方法
    impl Clone + Debug + PartialEq + Eq for WorkspaceError
    impl fmt::Display for WorkspaceError
    impl std::error::Error for WorkspaceError
```

# 逻辑

```text
启动：
    StartWorkspace
    -> create_workspace 创建 Workspace Entity 并登记 Registry
    -> 为每个成员发送 LoadAgentImage
    -> LoadAgentImageResult 经 collect_agent_image_system
    -> 打开 Memory，构造 AgentCreateRequest
    -> AgentCreateRequest 回执经 collect_agent_image_system 消费
    -> Workspace.agents 登记成功成员
    -> AgentInitializationCompleted 经 collect_agent_initialization_system 标记 Ready

停止：
    StopWorkspace / StopWorkspaceByReference / ReloadWorkspace
    -> stop_workspace_entity
    -> 停止并 despawn 所有 Agent
    -> 清理 pending_images / pending_agents / 模型路由
    -> despawn Workspace

消息：
    RouteAgentMessage -> route_agent_message_system -> AgentMessage
    RouteAgentTurnAbort -> route_agent_turn_abort_system -> AgentControl::AbortTurn
    RouteMclCommand -> route_mcl_command_system -> MclCommandRequest -> collect_mcl_command_response_system -> 外部回执
```

# 持有关系

```text
App
└── World
    ├── WorkspaceRegistry Resource
    │   ├── agent_images_root: Arc<PathBuf>
    │   ├── workspaces: Vec<Entity>
    │   ├── pending_images: BTreeMap<String, (Entity, String)>
    │   ├── pending_agents: BTreeMap<String, (Entity, String, oneshot::Receiver<...>)>
    │   └── pending_mcl_commands: BTreeMap<String, (mpsc::Sender<...>, oneshot::Receiver<...>)>
    └── Workspace Entity
        ├── ResourceId
        └── Workspace
            ├── definition: Arc<WorkspaceDefinition>
            ├── project_root: Arc<PathBuf>
            ├── manager_name: String
            ├── agents: BTreeMap<String, Entity>
            └── states: BTreeMap<String, WorkspaceAgentState>
