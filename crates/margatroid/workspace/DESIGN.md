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
update_workspace(context: &WorkspaceUpdateContext, change: WorkspaceChange) -> Result<WorkspaceChangeResult, WorkspaceError>
    行为：
        在WorkspaceDirectory更新门内校验来源及操作，提交Definitions/Register/State/Members/Failure/Remove变化并递增revision。
        定义变动只更新候选定义，不改运行实例的definition、成员、状态，不自动启动、停止或重启。
        状态或成员变动由各自真实生命周期入口调用本方法；原World扩展方法也只通过此入口修改。
        同步列表同路径优先展示当前有效候选定义；源文件删除后用运行实例保留的启动定义补位，仍允许停止。
        展示新候选定义不改运行实例执行材料；运行故障优先于定义读取故障，恢复只清对应定义故障，不覆盖真实生命周期错误。
        失败且无新可观察状态不发布；真实错误变化属于更新；释放锁后sync.publish，直接选择当前Clients并投递。

run_workspace_definition_monitor(context: WorkspaceWatchContext) -> Future<Output = Result<(), WorkspaceError>>
    行为：
        先挂notify观察后校验初始缓存，初始化及后续创建/修改/删除/rename按100ms安静窗口合并；忽略Access及无关文件。
        每次有效通知在至多一个blocking worker内调用同一全量load/reload，完整校验后调用update_workspace发布；不产生ECS同步事件。
        读取前后来源变动则继续稳定读取；重复内容不推进revision或发送，正在读取期间的新变化保留为待处理。
        非法定义保留最后有效缓存，通过既有Workspace.error形状呈现可定位的旧条目故障；没有可用身份的新非法文件只记录诊断，不伪造列表项或新wire字段。
        错误恢复清除错误并发布；取消后等待真实读取及notify退出，应用关停等待本监控收口。

WorkspaceDefinitions：定义目录与启动数据Resource
    entries: Arc<RwLock<BTreeMap<PathBuf, WorkspaceDefinition>>>
    source: Option<PathBuf>
    reload_lock: Arc<Mutex<()>>
    updates: WorkspaceUpdateContext--定义缓存和运行投影的唯一发布入口
    open(source: impl Into<PathBuf>, binding: WorkspaceSyncBinding) -> Result<Self, WorkspaceError>
        行为：规范来源并拒绝symlink，记录source后调用同一reload完成首轮全量加载。
    source(&self) -> Option<&Path>
        行为：借用已配置来源，供安装校验路径边界，不触发IO。
    get(&self, path: &Path) -> Option<WorkspaceDefinition>
        行为：从已发布缓存克隆定义；启动必须先reload成功并使用其返回快照。
    list(&self) -> Vec<(PathBuf, WorkspaceDefinition)>
        行为：短读锁克隆全部已发布定义列表，不隐式执行IO。
    reload(&self) -> Result<WorkspaceDefinitionSnapshot, WorkspaceError>
        行为：同步持有reload_lock调用load_workspace_definitions，读取source的全部Workspace定义并完整校验；异步调用方用blocking worker等待真实join。
            成功后调用update_workspace(Definitions)原子发布entries并通知workspace.sync，返回同一次发布的不可变快照；失败经同一入口记录可观察错误并返回失败，启动不得继续使用旧值。
            不提供按某个Workspace单独重载的入口；显式reload与启动/重启共享本方法，不改运行Entity或重启成员。
    start_watch(&self, context: WorkspaceWatchContext) -> WorkspaceDefinitionWatch
        行为：应用启动后为已配置来源启动一次notify监控，覆盖每个workspace.yaml及可检测新增/删除/原子替换的父目录；不是每帧创建。
    impl Clone for WorkspaceDefinitions
        行为：共享同一Arc，不复制缓存仓库。
    impl Resource for WorkspaceDefinitions

load_workspace_definitions(source: &Path) -> Result<BTreeMap<PathBuf, WorkspaceDefinition>, WorkspaceError>
    行为：拒绝symlink，扫描配置根下非隐藏子目录的workspace.yaml；单文件配置来源包含该文件全部定义，排序后完整解析并验证名称；任一失败不发布。
reject_definition_symlinks(path: &Path) -> Result<(), WorkspaceError>
    行为：逐路径分量拒绝symlink及metadata错误，允许尚未存在的目标由后续打开步骤报告。

WorldWorkspaceExt：工作区访问扩展，公开trait
    workspace_definitions(&self) -> Result<&WorkspaceDefinitions, WorkspaceError>
        行为：同步取得定义缓存能力，不执行文件IO。
    workspace_state(&self, workspace: Entity) -> Result<&WorkspaceState, WorkspaceError>
        行为：读取目标Workspace状态。
    workspace_agents(&self, workspace: Entity) -> Result<Vec<Entity>, WorkspaceError>
        行为：读取完整成员Entity列表。
    workspace_insert_agent(&mut self, workspace: Entity, agent: Entity) -> Result<(), WorkspaceError>
        行为：调用update_workspace(Members)加入成员，不读取Agent领域组件。
    workspace_remove_agent(&mut self, workspace: Entity, agent: Entity) -> Result<(), WorkspaceError>
        行为：调用update_workspace(Members)移除成员Entity。
    impl WorldWorkspaceExt for World
```

公开，定义于workspace_plugin：
```text
WorkspacePlugin：工作区编排插件，公开类型
    schedule: String
    definitions: WorkspaceDefinitions
    sync: WorkspaceSyncBinding
    open(root: impl Into<PathBuf>, sync: WorkspaceSyncBinding) -> Result<Self, WorkspaceError>
        行为：同步读取并校验root内全部Workspace定义；失败返回错误，成功构造插件，不创建运行Entity。
    impl Plugin for WorkspacePlugin
        build(self, app: &mut App)
            行为：检查运行设施及必需WorkspaceSyncBinding，插入definitions及共享WorkspaceDirectory，安装既有三个生命周期System。
                初始化完成后通过现有异步设施启动一次定义文件监控，归属应用生命周期；不增加同步System或按帧调查。
```

# components

## 类型

公开：
```text
Workspace：工作区运行实例，公开组件
    name: String
    definition_path: PathBuf
    project_root: PathBuf
    shared: Arc<WorkspaceRuntimeState>--状态、成员及故障通过统一入口修改
    definition: Option<WorkspaceDefinition>
    impl Component for Workspace
    行为：身份及本次启动定义保持实例自有，异步投影从共享运行信息读取，不借用World。

WorkspaceFailure(WorkspaceError)
    impl Component

WorkspaceExecutor：工作区专属线程控制，公开组件
    entry: WorkspaceExecutionEntry
    close: WorkspaceExecutionClose
    impl Component for WorkspaceExecutor
```

# events

## 类型

公开：
```text
WorkspaceOperationReply
    Arc<Mutex<Option<oneshot::Sender<Result<WorkspaceOperationOutput, WorkspaceError>>>>>

StartWorkspace
    id: String
    path: PathBuf
    reply: WorkspaceOperationReply
    impl Event + Clone + Debug

StopWorkspace
    id: String
    workspace: Entity
    reply: WorkspaceOperationReply
    impl Event + Clone + Debug

RestartWorkspace
    id: String
    path: PathBuf
    reply: WorkspaceOperationReply
    impl Event + Clone + Debug

```

# system

## 函数

crate公开：
```text
stop_workspace_system(world: &mut World)
    处理事件：StopWorkspace
    接收闭包结果：Result<StopOutput, StopError>
    提交闭包：stop_flow(WorkspaceFlowContext) -> Future<Output = Result<StopOutput, StopError>>
    产出事件：None
    oneshot回复：Result<WorkspaceOperationOutput, WorkspaceError>
    行为：StopWorkspace -> handle_stop_workspace；Result<StopOutput, StopError> -> handle_stop_result。

restart_workspace_system(world: &mut World)
    处理事件：RestartWorkspace
    接收闭包结果：Result<RestartOutput, RestartError>
    提交闭包：restart_flow(WorkspaceFlowContext) -> Future<Output = Result<RestartOutput, RestartError>>
    产出事件：None
    oneshot回复：Result<WorkspaceOperationOutput, WorkspaceError>
    行为：RestartWorkspace -> handle_restart_workspace；Result<RestartOutput, RestartError> -> handle_restart_result。

start_workspace_system(world: &mut World)
    处理事件：StartWorkspace
    接收闭包结果：Result<StartOutput, StartError>
    提交闭包：start_flow(WorkspaceFlowContext) -> Future<Output = Result<StartOutput, StartError>>
    产出事件：None
    oneshot回复：Result<WorkspaceOperationOutput, WorkspaceError>
    行为：StartWorkspace -> handle_start_workspace；Result<StartOutput, StartError> -> handle_start_result。

```

# handler

## 函数

crate公开：
```text
handle_start_workspace(world: &mut World, request: StartWorkspace)
    行为：保留原reply，校验request.path及重复启动，创建Starting实体并提交启动闭包；全量reload后目标必须存在，本次定义用于初始化Workspace.definition。

handle_stop_workspace(world: &mut World, request: StopWorkspace)
    行为：仅Running时同步转Stopping；关闭新任务及成员创建受理，提交关闭闭包；否则拒绝。

handle_restart_workspace(world: &mut World, request: RestartWorkspace)
    行为：按request.path定位运行Entity；仅Running且所有成员均无未完成工具任务时同步转Restarting并提交重启闭包；存在未完成任务则拒绝，不先关闭成员；否则拒绝。

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
handle_restart_result(world: &mut World, result: Result<RestartOutput, RestartError>)
    行为：核对完整Entity和对应状态；启动或重启成功转Running，关闭成功销毁Entity；失败清理完整时销毁，否则保留宿主；mount_executor将本次定义保存到Workspace.definition；失败时保存WorkspaceFailure，成功时清除故障，通过原oneshot回复领域结果。

```

私有：
```text
start_flow(context: WorkspaceFlowContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：调用fresh_definition取得本次全量重载缓存中的定义及成员参数；失败不启动。mount_executor后start_members，不再单独读取目标文件。

fresh_definition(context: &WorkspaceFlowContext) -> Future<Output = Result<(WorkspaceDefinition, Vec<AgentStartParameters>), WorkspaceError>>
    行为：owned definitions及path交blocking worker，调用reload并从返回缓存快照get目标定义；缺失明确失败，再调用prepare_definition。

prepare_definition(path: &Path, definition: WorkspaceDefinition) -> Result<(WorkspaceDefinition, Vec<AgentStartParameters>), WorkspaceError>
    行为：仅使用已从缓存取得的定义，核验成员目录和固定state.sqlite/setting.toml路径，构造原AgentStartParameters；不读取Workspace定义文件。

mount_executor(context, definition, state) / start_members(context, definition, parameters)
    行为：将本次定义保存到Workspace.definition，创建并挂载专属执行线程。
        为每个成员分别创建独立oneshot及StartAgent，逐条发送全部事件后使用join_all并发等待所有reply。
        一个StartAgent只携带一个成员及其参数，结果按原成员顺序收齐；任一成员失败仍等待其他成员收口。

stop_flow(context: WorkspaceFlowContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：等待已受理成员创建收口；逐成员发送StopAgent并等待reply，确认真实清理；通知执行线程退出并在线程外join；不销毁协调Entity。

restart_flow(context: WorkspaceFlowContext) -> Future<Output = Result<RestartOutput, RestartError>>
    行为：先fresh_definition全量重载并取本次缓存定义/参数，失败不关闭旧实例；成功后stop_members_and_join，mount_executor并start_members，全程Restarting；目标从新缓存移除时通过原reply失败，原实例保持。

```

# types

## 类型

私有：
```text
WorkspaceFlowContext：生命周期闭包材料
    id: String
    workspace: Entity
    events: RuntimeEventSender
    path: PathBuf
    definitions: WorkspaceDefinitions
    reply: WorkspaceOperationReply
    world: WorldClosureSender

StartOutput / StopOutput / RestartOutput：成功载荷
    workspace: Entity
    members: Vec<Entity>
    member_errors: Vec<MemberDiagnostic>
    reply: WorkspaceOperationReply

StartError / StopError / RestartError = FlowError
    workspace: Option<Entity>
    error: WorkspaceError
    cleanup_completed: bool
    restore_running: bool
    reply: WorkspaceOperationReply

MemberDiagnostic：成员诊断
```

# types pub

## 类型

公开：
```text

WorkspaceDirectory：共享Resource
    inner: Arc<WorkspaceDirectoryShared>
WorkspaceDirectoryShared
    update_gate: Mutex<()>
    definitions: Arc<RwLock<BTreeMap<PathBuf, WorkspaceDefinition>>>--与WorkspaceDefinitions.entries为同一缓存
    running: RwLock<BTreeMap<Entity, Arc<WorkspaceRuntimeState>>>
    revision: AtomicU64
    definition_errors: RwLock<BTreeMap<PathBuf, WorkspaceError>>
WorkspaceRuntimeState
    data: RwLock<WorkspaceRuntimeSnapshot>--私有，统一入口更新
WorkspaceRuntimeSnapshot
    name: String
    definition_path: PathBuf
    project_root: PathBuf
    state: WorkspaceState
    agents: BTreeSet<Entity>
    error: Option<WorkspaceError>
    definition: WorkspaceDefinition--该实例实际启动定义
    行为：访问器只提供只读值，编码无需World。
WorkspaceUpdateContext
    directory: Weak<WorkspaceDirectoryShared>
    sync: SyncContext<WorkspaceSyncSnapshot>
WorkspaceSyncSnapshot
    definitions: Vec<(PathBuf, WorkspaceDefinition)>
    running: Vec<WorkspaceRuntimeSnapshot>
    definition_errors: BTreeMap<PathBuf, WorkspaceError>
WorkspaceChange
    Definitions(WorkspaceDefinitionSnapshot) | DefinitionError { path: PathBuf, error: WorkspaceError }
    Register(WorkspaceRuntimeSnapshot) | State { workspace: Entity, state: WorkspaceState }
    Members { workspace: Entity, agents: BTreeSet<Entity> } | Failure { workspace: Entity, error: WorkspaceError }
    Remove(Entity)
WorkspaceChangeResult
    Unit | Definitions(WorkspaceDefinitionSnapshot)
WorkspaceSyncBinding
    directory: WorkspaceDirectory
    updates: WorkspaceUpdateContext
WorkspaceDefinitionWatch
    cancel: CancellationToken
    completion: 真实监控JoinHandle
WorkspaceWatchContext
    source: PathBuf
    definitions: WorkspaceDefinitions
    updates: WorkspaceUpdateContext
    cancel: CancellationToken

WorkspaceState：生命周期状态，公开枚举
    Starting
    Running
    Restarting
    Stopping

WorkspaceDefinitionSnapshot：全量重载成功后的不可变缓存快照，公开纯值类型
    entries: Arc<BTreeMap<PathBuf, WorkspaceDefinition>>
    get(&self, path: &Path) -> Option<&WorkspaceDefinition>
        行为：按规范路径借用本次已发布定义，不回源文件，不借用可变目录锁。
    list(&self) -> Vec<(PathBuf, WorkspaceDefinition)>
        行为：按路径顺序克隆本次快照全部条目。

WorkspaceDefinition：持久定义，公开类型
    name: String
    project_root: PathBuf
    members: Vec<WorkspaceMemberDefinition>

WorkspaceMemberDefinition：成员定义，公开类型
    name: String
    driver: String
    home_root: PathBuf

WorkspaceOperationOutput
    workspace: Entity
    members: Vec<Entity>
    member_errors: Vec<MemberDiagnostic>

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
