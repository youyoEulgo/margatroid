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
WorkspaceDefinitions：Resource + Clone
    entries: Arc<RwLock<BTreeMap<PathBuf, WorkspaceDefinition>>>--私有，不借出可变缓存
    source: Option<PathBuf>
    reload_lock: Arc<Mutex<()>>
    observer: Arc<OnceLock<DefinitionObserver>>
    open(source: impl Into<PathBuf>) -> Result<Self, WorkspaceError>
        行为：验证来源并首轮全量reload，失败不安装；不自动运行Workspace。
    source(&self) -> Option<&Path>
    same_instance(&self, other: &Self) -> bool
    get(&self, path: &Path) -> Option<WorkspaceDefinition>
    list(&self) -> Vec<(PathBuf, WorkspaceDefinition)>
        行为：只读取已发布缓存，不隐式IO或建立监控。
    observe(&self, observer: DefinitionObserver)
        行为：运行前仅装配一次typed观察，磁盘异步监控由DTO提供。
    reload(&self) -> Result<WorkspaceDefinitionSnapshot, WorkspaceError>
        行为：reload_lock覆盖读取、完整校验和发布；成功整体替换entries并调用observer(Ok(snapshot))，失败保留旧缓存并observer(Err(error))。
        显式reload、启动/重启和磁盘监控共用此入口；返回不可变快照，运行实例仍保留自己启动使用的definition。
WorkspaceDefinitionSnapshot
    entries: Arc<BTreeMap<PathBuf, WorkspaceDefinition>>
    get(&self, path: &Path) -> Option<&WorkspaceDefinition>
    list(&self) -> Vec<(PathBuf, WorkspaceDefinition)>
WorldWorkspaceExt
    workspace_definitions(&self) -> Result<&WorkspaceDefinitions, WorkspaceError>
    workspace_state(&self, workspace: Entity) -> Result<&WorkspaceState, WorkspaceError>
    workspace_agents(&self, workspace: Entity) -> Result<Vec<Entity>, WorkspaceError>
    workspace_insert_agent(&mut self, workspace: Entity, agent: Entity) -> Result<(), WorkspaceError>
    workspace_remove_agent(&mut self, workspace: Entity, agent: Entity) -> Result<(), WorkspaceError>
        行为：成员写入只调用update_workspace的AddMember/RemoveMember，原完整Entity校验保持。
load_workspace_definitions(source: &Path) -> Result<BTreeMap<PathBuf, WorkspaceDefinition>, WorkspaceError>
reject_definition_symlinks(path: &Path) -> Result<(), WorkspaceError>
    行为：沿原路径规则加载全部workspace.yaml，拒绝不允许的symlink及不合法定义，任一错误不发布部分结果。
```

公开，定义于workspace_plugin：
```text
WorkspacePlugin
    schedule: String
    definitions: WorkspaceDefinitions
    program_root: PathBuf--Workspace定义程序的明确资源根
    state: WorkspaceState--本次异步阶段
    startup: Arc<StartupCompletion>--在途启动真实完成屏障
    root: Option<PathBuf>
    file: Option<PathBuf>
    new() -> Self
    open(root: impl Into<PathBuf>) -> Result<Self, WorkspaceError>
    with_schedule(self, schedule: impl Into<String>) -> Self
    impl Default + Plugin
        build(self, app: &mut App)
            行为：检查运行设施及必需WorkspaceUpdateContext，安装唯一definitions Resource和既有Stop/Restart/Start三个System。
            定义监控由DTO在其安装阶段启动一次，不隐藏在World帧轮询中。
```

# components

## 类型

公开：
```text
Workspace：Component + Clone + Debug
    data: WorkspaceData--私有，不实现DerefMut
    updates: WorkspaceUpdateContext
    new(name, definition_path, project_root, state, agents, definition, updates) -> Self
    updates(&self) -> WorkspaceUpdateContext
    impl Deref<Target = WorkspaceData>：只读字段访问
WorkspaceData
    name: String
    definition_path: PathBuf
    project_root: PathBuf--运行实例实际工作目录；HTTP文件访问只以此目录为根，不根据同名展示条目或新定义缓存替换
    state: WorkspaceState
    agents: BTreeSet<Entity>
    definition: Option<WorkspaceDefinition>
    error: Option<WorkspaceError>
WorkspaceFailure(WorkspaceError)
    impl Component
    行为：原运行诊断组件保留，实际故障投影由统一入口同步维护。
WorkspaceExecutor
    entry: WorkspaceExecutionEntry
    close: WorkspaceExecutionClose
    impl Component
    impl Drop：关闭外部新提交受理，不冒充真实线程退出。
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
    接收闭包结果：None
    提交闭包：等待StopWorkspace原oneshot的普通异步闭包，返回Result<(),AsyncTaskError>
    闭包产出事件：停止成功后一次StartWorkspace
    oneshot回复：停止失败直接回复原RestartWorkspace，成功由普通StartWorkspace完成原reply
    行为：仅路由RestartWorkspace -> handle_restart_workspace，不接收独立RestartOutput或执行第二套生命周期。

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
update_workspace(world: &mut World, owner: Entity, change: WorkspaceChange) -> Result<(), WorkspaceError>
    行为：唯一修改Workspace私有data的入口，统一处理状态、成员、故障、实际启动定义；变化后释放可变借用再context.notify，DTO探针提交owned运行快照并直投Clients。
remove_workspace(world: &mut World, owner: Entity)
    行为：真实停止收口后销毁Entity，再通知完整目录更新；不以提前移除代替回收。
WorkspaceState：Starting | Running | Error | Stopping，无Restarting；定义存在但无运行Entity时网络目录可投影inactive。

handle_start_workspace(world: &mut World, request: StartWorkspace)
    行为：保留原reply，校验request.path及重复启动，创建Starting实体并提交启动闭包；全量reload后目标必须存在，本次定义用于初始化Workspace.definition。

handle_stop_workspace(world: &mut World, request: StopWorkspace)
    行为：Starting/Running/Error均同步转Stopping；关闭新任务及成员创建受理，并立即取消本Workspace实际Lua执行，使未完成启动交接能收口。
        stop_flow仍等待StartupBarrier真实完成，再按原路径停止Lua、Agent和线程；已Stopping拒绝重复提交，由原监督者等待收口。

handle_restart_workspace(world: &mut World, request: RestartWorkspace)
    行为：按request.path定位运行Entity，直接调用handle_stop_workspace，使用普通StopWorkspace的受理/取消/初始化等待及真实清理。
        仅await StopWorkspace.reply；成功后提交普通StartWorkspace，携带原path、id和原reply。停止失败/回执丢失不启动；不查询ToolTasks、不提前reload定义、不额外改状态/装Barrier、不保留旧Entity。
        定义全量reload及新Entity/线程/Driver创建仅发生在普通StartWorkspace路径。

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
    行为：核对完整Entity和对应状态；启动成功转Running，关闭成功销毁Entity；失败清理完整时销毁，否则保留宿主并转Error；mount_executor将本次定义保存到Workspace.definition；失败时保存WorkspaceFailure；成员Lua启动失败先发布Error并通过原reply返回聚合MemberFailed，不发布Running；全成功才清除旧故障并Running。

```

私有：
```text
start_flow(context: WorkspaceFlowContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：调用fresh_definition取得本次全量重载缓存中的定义；失败不启动。mount_executor后start_members，不再单独读取目标文件。

fresh_definition(context: &WorkspaceFlowContext) -> Future<Output = Result<WorkspaceDefinition, WorkspaceError>>
    行为：owned definitions及path交blocking worker，调用reload并从返回缓存快照get目标定义；缺失明确失败，再调用prepare_definition。

prepare_definition(path: &Path, definition: WorkspaceDefinition) -> Result<WorkspaceDefinition, WorkspaceError>
    行为：仅使用已从缓存取得的定义，核验成员目录和固定state.sqlite/setting.toml路径，构造原AgentStartParameters；不读取Workspace定义文件。

mount_executor(context, definition, state) / start_members(context, definition)
    行为：将本次定义保存到Workspace.definition，创建并挂载专属执行线程。
        为每个定义创建独立执行Entity和WorkspaceLuaTask，发送StartDriverLuaVm并保留原oneshot，join_all等待全部启动交接。
        一个StartDriverLuaVm只携带一个程序及其注入环境，结果按原成员顺序收齐；任一Lua任务失败仍等待其他启动交接收口。

stop_flow(context: WorkspaceFlowContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：等待已受理创建收口，先关闭新提交并逐个停止WorkspaceLuaTask对应实际执行，等待StopDriverLuaVm原oneshot及ended；移除独立执行Entity。
        再经World逐个核对该Workspace所有Agent；任务scope清理已移除的成员跳过，Stopping成员等待Agent::removal，其他成员提交原StopAgent并等待reply及实际移除；不存在owner树；Agent工具任务scope负责其创建实例的清理；最后执行线程退出并在线程外join，不销毁协调Entity。


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
    program_root: PathBuf--Workspace定义程序的明确资源根
    state: WorkspaceState--本次异步阶段
    startup: Arc<StartupCompletion>--在途启动真实完成屏障
    reply: WorkspaceOperationReply
    world: WorldClosureSender

StartOutput / StopOutput：成功载荷
    workspace: Entity
    members: Vec<Entity>
    member_errors: Vec<MemberDiagnostic>
    reply: WorkspaceOperationReply

StartError / StopError = FlowError
    workspace: Option<Entity>
    error: WorkspaceError
    cleanup_completed: bool
    state: Option<WorkspaceState>--只允许当前匹配阶段处理失败，停止不能被旧启动结果覆盖
    reply: WorkspaceOperationReply

StartupCompletion：私有共享启动完成证明
    done: AtomicBool
    changed: tokio::sync::Notify
    finish(&self)：真实启动Future完成后发布并通知
    wait(&self)：先注册后检查done，停止先等待所有启动工作真实收口
StartupBarrier(Arc<StartupCompletion>)：Component
    行为：仅初始创建安装，guarded_start完成后finish；stop_flow先wait，避免晚到程序创建落在关闭快照之后。
MemberDiagnostic：成员诊断
```

# types pub

## 类型

公开：
```text

WorkspaceLuaTask（lua_runtime_core组件）
    workspace: Entity
    name: String
    行为：管理Workspace定义自动启动的Lua执行实体；与Workspace所属Agent列表分开。停止先关闭新增创建受理，取消所属执行并真实等待，再排空所属Agent及执行线程。
WorkspaceMemberDefinition
    name: String
    driver: String--公开程序locator，解析到base.lua，不要求manifest
    home_root: PathBuf
    行为：不预创建Agent；Workspace准备读取沿read_driver原预算及blocking worker，注入workspace/name/resource_dir，公共Lua创建方法按需发送StartAgent。

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
