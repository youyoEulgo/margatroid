# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，不创建空模块。
类型、函数使用二级标题；列出字段、签名及直接行为。
System分别列出处理事件、接收闭包结果、提交闭包与产出事件，使用Rust类型和函数名；空项使用None。
不写Rust Attribute；持有关系使用树形图。
```

# 板块格式

```text
# lib        workspace/core/src/lib.rs；workspace/plugin/src/lib.rs  Workspace Resource与扩展；Plugin安装
# components workspace/core/src/components.rs                        Workspace与执行器组件
# events     workspace/core/src/events.rs                            工作区生命周期契约
# system     workspace/plugin/src/system.rs                          事件收集与路由
# handler    workspace/plugin/src/handler.rs                         生命周期编排
# types      workspace/plugin/src/types.rs                           闭包参数与输出
# types pub  workspace/core/src/types.rs                             状态、定义与线程能力
# error      workspace/core/src/error.rs                             Workspace错误
```

# lib

## 类型

公开，定义于workspace_core：
```text
WorkspaceDefinitions：展示配置Resource
    entries: Arc<RwLock<BTreeMap<PathBuf, WorkspaceDefinition>>>--路径到展示缓存，共享给启动闭包
    get(&self, path: &Path) -> Option<WorkspaceDefinition>
        行为：短读锁克隆展示值，不供启动使用。
    list(&self) -> Vec<(PathBuf, WorkspaceDefinition)>
        行为：短读锁克隆路径和展示缓存列表。
    impl Clone for WorkspaceDefinitions
        行为：共享同一Arc，不复制缓存仓库。
    impl Resource for WorkspaceDefinitions

WorldWorkspaceExt：工作区访问扩展，公开trait
    workspace_definitions(&self) -> Result<&WorkspaceDefinitions, WorkspaceError>
        行为：同步读取路径及展示缓存，不执行文件IO。
    workspace_state(&self, workspace: Entity) -> Result<&WorkspaceState, WorkspaceError>
        行为：读取目标Workspace状态。
    workspace_agents(&self, workspace: Entity) -> Result<Vec<Entity>, WorkspaceError>
        行为：读取完整成员Entity列表。
    workspace_insert_agent(&mut self, workspace: Entity, agent: Entity) -> Result<(), WorkspaceError>
        行为：将Entity加入成员集合，不读取Agent领域组件。
    workspace_remove_agent(&mut self, workspace: Entity, agent: Entity) -> Result<(), WorkspaceError>
        行为：移除成员Entity。
    impl WorldWorkspaceExt for World
```

公开，定义于workspace_plugin：
```text
WorkspacePlugin：工作区编排插件，公开类型
    schedule: String
    definitions: WorkspaceDefinitions--私有，安装前加载的路径及展示缓存
    open(root: impl Into<PathBuf>) -> Result<Self, WorkspaceError>
        行为：同步读取并校验root内全部Workspace定义；失败返回错误，成功构造插件，不创建运行Entity。
    impl Plugin for WorkspacePlugin
        build(self, app: &mut App)
            行为：检查运行设施并拒绝重复安装；将definitions作为Resource插入World；在同一Schedule依次安装stop_workspace_system、restart_workspace_system、start_workspace_system。
```

# components

## 类型

公开：
```text
Workspace：工作区运行实例，公开组件
    name: String
    definition_path: PathBuf--运行实例对应的定义文件，不从展示缓存解析身份
    project_root: PathBuf
    state: WorkspaceState
    agents: BTreeSet<Entity>
    impl Component for Workspace

WorkspaceExecutor：工作区专属线程控制，公开组件
    entry: WorkspaceExecutionEntry--可克隆提交入口
    close: WorkspaceExecutionClose--关闭及join能力
    impl Component for WorkspaceExecutor
```

# events

## 类型

公开：
```text
StartWorkspace：启动请求，公开事件
    id: String
    path: PathBuf--本次必须重新读取的定义文件
    impl Event for StartWorkspace

StopWorkspace：关闭请求，公开事件
    id: String
    workspace: Entity
    impl Event

RestartWorkspace：重启请求，公开事件
    id: String
    path: PathBuf--与StartWorkspace相同
    impl Event for RestartWorkspace

StartWorkspaceResult / StopWorkspaceResult / RestartWorkspaceResult：操作结果，公开事件
    id: String
    result: Result<WorkspaceOperationOutput, WorkspaceError>
    impl Event

```

# system

## 函数

crate公开：
```text
stop_workspace_system(world: &mut World)
    处理事件：StopWorkspace
    接收闭包结果：Result<StopOutput, StopError>
    提交闭包：stop_flow(WorkspaceFlowContext) -> Future<Output = Result<StopOutput, StopError>>
    产出事件：StopWorkspaceResult
    行为：StopWorkspace -> handle_stop_workspace；Result<StopOutput, StopError> -> handle_stop_result。

restart_workspace_system(world: &mut World)
    处理事件：RestartWorkspace
    接收闭包结果：Result<RestartOutput, RestartError>
    提交闭包：restart_flow(WorkspaceFlowContext) -> Future<Output = Result<RestartOutput, RestartError>>
    产出事件：RestartWorkspaceResult
    行为：RestartWorkspace -> handle_restart_workspace；Result<RestartOutput, RestartError> -> handle_restart_result。

start_workspace_system(world: &mut World)
    处理事件：StartWorkspace
    接收闭包结果：Result<StartOutput, StartError>
    提交闭包：start_flow(WorkspaceFlowContext) -> Future<Output = Result<StartOutput, StartError>>
    产出事件：StartWorkspaceResult
    行为：StartWorkspace -> handle_start_workspace；Result<StartOutput, StartError> -> handle_start_result。

```

# handler

## 函数

crate公开：
```text
handle_start_workspace(world: &mut World, request: StartWorkspace)
    行为：校验request.path并检查重复启动；创建Starting实体和基础组件，提交启动闭包；不读取展示缓存作为启动材料。

handle_stop_workspace(world: &mut World, request: StopWorkspace)
    行为：仅Running时同步转Stopping；关闭新任务及成员创建受理，提交关闭闭包；否则拒绝。

handle_restart_workspace(world: &mut World, request: RestartWorkspace)
    行为：按request.path定位运行Entity；仅Running且所有成员均无未完成工具任务时同步转Restarting并提交重启闭包；存在未完成任务则拒绝，不先关闭成员；否则拒绝。

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
handle_restart_result(world: &mut World, result: Result<RestartOutput, RestartError>)
    行为：核对完整Entity和对应状态；启动或重启成功转Running，关闭成功销毁Entity；失败清理完整时销毁，否则保留宿主；回复原请求。

```

私有：
```text
start_flow(context: WorkspaceFlowContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：按context.path重新读取并解析定义，更新展示缓存并将同一本次解析值直接用于启动；创建专属执行线程；逐成员发送StartAgent并等待reply；收集成功Entity及成员失败。

stop_flow(context: WorkspaceFlowContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：等待已受理成员创建收口；逐成员发送StopAgent并等待reply，确认真实清理；通知执行线程退出并在线程外join；不销毁协调Entity。

restart_flow(context: WorkspaceFlowContext) -> Future<Output = Result<RestartOutput, RestartError>>
    行为：先按context.path读取并校验定义，更新展示缓存；失败不关闭旧实例；成功后await stop_flow，再使用本次解析值执行启动步骤；全程Restarting，不二次从缓存取值。

```

# types

## 类型

私有：
```text
WorkspaceFlowContext：生命周期闭包材料
    id: String
    workspace: Entity
    events: RuntimeEventSender
    path: PathBuf--请求携带的定义文件路径
    definitions: WorkspaceDefinitions--克隆的Arc缓存句柄，不借用World

StartOutput / StopOutput / RestartOutput：成功载荷
    id: String
    workspace: Entity
    members: Vec<Entity>
    member_errors: Vec<MemberDiagnostic>

StartError / StopError / RestartError：失败载荷
    id: String
    workspace: Entity
    error: WorkspaceError
    cleanup_completed: bool

MemberDiagnostic：成员诊断--成员名称和转换后的WorkspaceError
```

# types pub

## 类型

公开：
```text

WorkspaceState：生命周期状态，公开枚举
    Starting
    Running
    Restarting
    Stopping

WorkspaceDefinition：持久定义，公开类型
    name: String
    project_root: PathBuf
    members: Vec<WorkspaceMemberDefinition>

WorkspaceMemberDefinition：成员定义，公开类型
    name: String
    driver: String--未加载的文件定位输入
    home_root: PathBuf

WorkspaceOperationOutput：生命周期输出，公开类型--Workspace Entity、成员Entity及成员诊断

WorkspaceExecutionEntry：线程任务提交能力，公开可克隆类型
    submit(&self, task: impl FnOnce() -> LocalFuture + Send + 'static) -> Result<(), WorkspaceError>
        行为：检查受理门并入队，在专属线程创建和运行可非Send的Future。
    close_admission(&self)
        行为：同步关闭普通任务提交，不停止已有任务。

WorkspaceExecutionClose：线程关闭能力，公开类型
    close_and_join(&self) -> Future<Output = Result<(), WorkspaceError>>
        行为：通知线程退出并异步等待join；失败返回真实线程错误。

spawn_workspace_executor() -> Result<WorkspaceExecutor, WorkspaceError>
    行为：创建专属线程和本地异步执行环境，返回控制组件。
```

# error

## 类型

公开：
```text
WorkspaceError：工作区错误，公开类型
    kind: WorkspaceErrorKind
    message: String
    impl Display + std::error::Error

WorkspaceErrorKind：错误类别，公开枚举
    InvalidRequest | InvalidDefinition | Missing | Duplicate | StateRejected | Busy
    MemberFailed | ThreadStartFailed | ThreadCloseFailed | CleanupFailed
```

# 持有关系

```text
World
├── WorkspaceDefinitions: Resource
│   └── Arc<RwLock<BTreeMap<PathBuf, WorkspaceDefinition>>>
└── Workspace Entity
    ├── Address（address）
    ├── Workspace（workspace_core）
    │   └── BTreeSet<Agent Entity>
    └── WorkspaceExecutor（workspace_core）
        ├── WorkspaceExecutionEntry
        └── WorkspaceExecutionClose
            └── 专属线程join能力

mecs全局异步设施
└── Workspace生命周期闭包
    ├── owned参数、Entity及事件发送能力
    └── 各步骤oneshot接收端

Workspace专属线程
└── 已提交的Driver等本地任务
```
