# 伪代码格式

```text
板块：一级标题，一行注明core与plugin文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列出字段；函数列出签名及直接行为。
System分别列出处理事件、接收闭包结果、提交闭包与产出事件，使用Rust类型和函数名；空项使用None。
属性不写Rust Attribute；持有关系使用树形图。
方法使用下列层级；--仅承载简短说明，执行逻辑写入行为。
method(args) -> xxx
    行为：
        xxx
```

# 板块定义

```text
# lib        agent/core/src/lib.rs；agent/plugin/src/lib.rs  Agent自身扩展；Plugin安装
# components agent/core/src/components.rs                    Agent组件
# events     agent/core/src/events.rs                        启停、重启及异常事件
# system     agent/plugin/src/system.rs                      事件收集与路由
# handler    agent/plugin/src/handler.rs                     同步受理、异步流程与结果收口
# types      agent/plugin/src/types.rs                       流程参数与闭包输出
# types pub  agent/core/src/types.rs                         AgentState与启动参数
# error      agent/core/src/error.rs                         AgentError
```

# lib

## 类型

```text
-- 公开，定义于agent_core
WorldAgentExt：Agent访问扩展，公开trait
    agent_state(&self, agent: Entity) -> Result<AgentState, AgentError>
        行为：
            读取Agent状态，目标不存在返回错误。
    agent_start_turn(&mut self, agent: Entity) -> Result<String, AgentError>
        行为：
            仅Idle允许；
            同步生成turn并转Working。
    agent_finish_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>
        行为：
            仅Working且本次推理调用链已收口时转Idle，不等待已return未finish的后台工具任务；
            未return工具的中止结果先交付，不以其自然结束作为Driver收尾条件；
            并清空turn；
            有活动turn时核对匹配，中止后turn为空由当前Driver调用链完成；
            调用方plugin核对VM实例，拒绝旧实例完成。
    agent_abort_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>
        行为：
            仅Working且turn匹配时同步清空turn，保持Working；
            否则返回错误。此操作必须先于同帧MCL、推理和工具调用受理；
            不在agent_core跨域取消任务，调用方随后通过各领域core执行推理取消和ToolTasks未return任务的原子强制return及清理。
    impl WorldAgentExt for World
        行为：
            只访问Agent自身共享信息；start_turn/finish_turn/abort_turn经同一AgentUpdateContext调用update_agent，不能直接修改字段或旁路同步。
```

```text
-- 公开，定义于agent_plugin
AgentPlugin：生命周期插件，公开类型
    schedule: String--启停System所属计划
    impl Default for AgentPlugin
        行为：
            使用RuntimePlugin::UPDATE。
    impl Plugin for AgentPlugin
        build(self, app: &mut App)
            行为：
                检查运行设施和计划，拒绝重复安装。
                在同一schedule依次add_system：stop_agent_system、restart_agent_system、start_agent_system。
                安装异常System；
                各启停System收集自己的闭包结果。
                使用mecs按挂载顺序同步执行的机制，不增加生命周期排序器或统一请求枚举。
```

# components

## 类型

```text
-- 公开
Agent：Agent领域组件，公开结构体
    name: String
    workspace: Entity
    shared: Arc<AgentShared>--state、turn、故障和公共信息只读，写入必须经过update_agent
    impl Component for Agent

AgentDriverSource
    address: String
    manifest_path: PathBuf
    impl Component

```

# events

## 类型

```text
-- 公开
StartAgent：启动请求，公开事件
    id: String
    workspace: Entity
    name: String
    parameters: AgentStartParameters
    reply: Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>
    impl Event for StartAgent

StopAgent：关闭请求，公开事件
    id: String
    agent: Entity
    reply: Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>
    impl Event for StopAgent

RestartAgent：重启请求，公开事件
    id: String
    workspace: Entity
    name: String
    parameters: AgentStartParameters
    reply: Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>
    impl Event for RestartAgent

AgentException：异常输入，公开事件
    id: String
    agent: Entity
    error: AgentError
    impl Event for AgentException
```

# system

## 函数

```text
-- crate公开
stop_agent_system(world: &mut World)
    处理事件：StopAgent
    接收闭包结果：Result<StopOutput, StopError>
    提交闭包：stop_flow(FlowContext) -> Future<Output = Result<StopOutput, StopError>>
    产出事件：None
    oneshot回复：Result<Entity, AgentError>
    行为：
        StopAgent -> handle_stop_agent；
        Result<StopOutput, StopError> -> handle_stop_result。

restart_agent_system(world: &mut World)
    处理事件：RestartAgent
    接收闭包结果：Result<RestartOutput, RestartError>
    提交闭包：restart_flow(FlowContext) -> Future<Output = Result<RestartOutput, RestartError>>
    闭包产出事件：InitToolTasks、InitInference、StartDriverLuaVm、StopDriverLuaVm
    产出事件：None
    oneshot回复：Result<Entity, AgentError>
    行为：
        RestartAgent -> handle_restart_agent；
        Result<RestartOutput, RestartError> -> handle_restart_result。

start_agent_system(world: &mut World)
    处理事件：StartAgent
    接收闭包结果：Result<StartOutput, StartError>
    提交闭包：start_flow(FlowContext) -> Future<Output = Result<StartOutput, StartError>>
    闭包产出事件：InitToolTasks、InitInference、StartDriverLuaVm
    产出事件：None
    oneshot回复：Result<Entity, AgentError>
    行为：
        StartAgent -> handle_start_agent；
        Result<StartOutput, StartError> -> handle_start_result。

agent_exception_system(world: &mut World)
    处理事件：AgentException
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    行为：
        AgentException -> handle_agent_exception。
```

# handler

## 函数

```text
-- crate公开
update_agent(context: &AgentUpdateContext, change: AgentChange) -> Result<AgentChangeResult, AgentError>
    行为：
        取得AgentDirectory更新门，验证完整Entity和来源epoch，在同一门内更新实际AgentShared及公共目录revision。
        Register/Transition/StartTurn/FinishTurn/AbortTurn/Fault/Tools/Sandboxes/Model/Remove均从此入口提交，Agent公共字段不提供外部可变借用。
        StartTurn仅Idle可受理，在同一临界区生成turn并转Working；FinishTurn核对当前调用链收口，清turn并转Idle；AbortTurn核对turn后先清turn并保持Working。
        各状态白名单、原reply和跨域取消顺序保持；拒绝或值未变不发布。
        释放领域锁后直接发布agent.sync；turn实际变化时再调用该Agent的turn SyncContext，均使用同一Clients槽位。
        Remove只在实际清理收口后移出目录并发布完整Agent列表，同时关闭该实例的per-Agent状态来源，迟到任务不复活旧对象。

handle_start_agent(world: &mut World, request: StartAgent)
    行为：
        校验Workspace和成员创建请求；
        同步创建Agent Entity并经update_agent(Register)登记Starting公共视图，再提交start_flow；
        拒绝时发送启动错误。

handle_stop_agent(world: &mut World, request: StopAgent)
    行为：
        通过update_agent按白名单同步转Stopping并直接发布agent.sync；
        关闭新请求受理，再提交stop_flow；
        拒绝时发送关闭错误。

handle_restart_agent(world: &mut World, request: RestartAgent)
    行为：
        在request.workspace内按name定位成员Entity，先检查ToolTasks，存在尚未finish的活任务条目直接拒绝；
        通过update_agent按白名单转Restarting，关闭新请求受理并同步公共状态；
        使用request.parameters提交restart_flow；
        失败通过原reply回复。

handle_agent_exception(world: &mut World, event: AgentException)
    行为：
        核对Entity，通过update_agent写入故障及关闭新受理，直接发布agent.sync；不发送DTO同步事件。

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
    行为：
        核对Entity及对应生命周期状态；
        忽略失效结果。
        启动成功：经update_agent转Idle，通过原StartAgent.reply回复Entity。
        失败：
            有完整清理证明则经update_agent(Remove)移出公共目录再销毁Entity，否则保留宿主及当前生命周期状态。
            通过原reply回复错误。

handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
    行为：
        核对Entity及对应生命周期状态；
        忽略失效结果。
        关闭成功：经update_agent(Remove)移出公共目录，再销毁Entity，通过原StopAgent.reply回复原Entity。
        失败：
            有完整清理证明则经update_agent(Remove)移出公共目录再销毁Entity，否则保留宿主及当前生命周期状态。
            通过原reply回复错误。

handle_restart_result(world: &mut World, result: Result<RestartOutput, RestartError>)
    行为：
        核对Entity及对应生命周期状态；
        忽略失效结果。
        重启成功：经update_agent转Idle，通过原RestartAgent.reply回复Entity。
        失败：
            有完整清理证明则经update_agent(Remove)移出公共目录再销毁Entity，否则保留宿主及当前生命周期状态。
            通过原reply回复错误。

```

```text
-- 私有
start_flow(context: FlowContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：
        调用address::resolve_file及agent_driver_core::read_driver取得本次Driver材料，准备成员目录中的state.sqlite与setting.toml；仅接受这两个固定文件名，不读取旧名。
        完成Driver读取后，通过实际来源安装入口绑定新的UI观察上下文，使用Driver实际清单路径启动文件监控；不等下一帧扫描组件。
        AgentStorage初始化后，使用注入的StorageSyncBinding启动该实际存储的持续提交监控。
        上述监控由start_flow已有异步运行上下文启动，持有各自取消及真实join能力，登记到本次来源租约；不新增执行器或每帧调查。
        通过原有生命周期World闭包安装AgentDriverSource及租约；来源替换先使旧source失效并取消、等待旧监控，再发布新来源。
        运行组件初始化阶段先发送InitToolTasks和InitInference，各携带agent及独立oneshot发送端，不先等待其中一个再发另一个。
        使用tokio::join!等待两个接收端，与原有资源初始化一起完成；
        收齐全部结果后处理错误，不提前丢弃另一项。
        新实例先从AgentUpdateContext构造Tools、沙箱和模型的typed on_change回调，再创建对应资源及AgentInferenceConfig，不允许默认缺失同步上下文。
        所有导入/动态注册/移除经Tool资源统一写入口更新Agent公共Tools视图；沙箱及模型公共投影也由各自成功更新入口提交。全部初始化成功后准备agent_info并发送StartDriverLuaVm，等待原reply；
        agent_info作为共享margatroid.context的只读材料，driver:mcl和driver:wait由Lua Runtime构造
        Driver及工具宿主克隆已初始化的AgentStorage作为日志能力，不增加启动事件字段
        失败清理本次已创建内容并返回，成功返回；
        core不引用其他core的组件类型。

stop_flow(context: FlowContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：
        请求Driver停止和执行取消；
        沿现有流程停止任务；
        任务监督移除所属Tools记录；其他任务按Agent关闭的既有停止流程清理，不按工具owner关系级联。
        等待任务、子进程及MCL真实收口，并等待本次已提交的日志写入完成后再释放存储句柄
        在释放实际来源前取消UI及持久记录监控并等待真实读取worker和监控Future退出；通过生命周期入口移除AgentDriverSource并失效旧source，清理前仍运行的Driver保留原来源
        返回清理结果，不销毁协调Entity。

restart_flow(context: FlowContext) -> Future<Output = Result<RestartOutput, RestartError>>
    行为：
        await stop_flow，成功后使用重启请求提供的parameters执行start_flow；
        全程Restarting，不发公开StopAgent或StartAgent事件。
```

# types

## 类型

```text
-- 私有
FlowContext：闭包上下文
    id: String--原请求ID
    agent: Entity
    events: RuntimeEventSender
    parameters: Option<AgentStartParameters>--启动与重启为Some，关闭为None
    sync: AgentUpdateContext
    task_sync: TaskSyncBinding
    ui_sync: DriverUiSyncBinding
    storage_sync: StorageSyncBinding--各自所属core的能力，来源prepare/encode由compose注入
    observations: 本次来源的UI与记录监控取消/等待能力
    reply: Option<oneshot::Sender<Result<Entity, AgentError>>>--原请求回复端

StartOutput / StopOutput / RestartOutput：对应闭包的成功载荷
    id: String--原请求ID
    agent: Entity

StartError / StopError / RestartError：对应闭包的失败载荷
    id: String--原请求ID
    agent: Entity
    error: AgentError
    cleanup_completed: bool

```

# types pub

## 类型

```text
-- 公开
AgentDirectory：Resource，可克隆共享信息
    inner: Arc<AgentDirectoryShared>
AgentDirectoryShared
    update_gate: Mutex<()>
    entries: RwLock<BTreeMap<Entity, Arc<AgentShared>>>
    revision: AtomicU64
AgentShared
    data: RwLock<AgentPublicState>--字段私有，不借出可变引用
AgentPublicState
    address: String
    name: String
    workspace: String
    state: AgentState
    turn: Option<String>
    error: Option<AgentError>
    tools: Vec<AgentToolInfo>
    sandboxes: Vec<AgentSandboxInfo>
    sandbox: Option<String>
    model: Option<AgentModelInfo>
    行为：Tools/沙箱/模型是用于对外观察的派生纯值，由对应资源写入口同步更新，不能独立编辑为第二份业务事实；模型不含凭据。
AgentToolInfo
    alias: String
    path: String
    description: String
    input_schema: serde_json::Value
    owner_task_id: Option<String>
    binding: Option<serde_json::Value>
AgentSandboxInfo
    alias: String
    path: String
AgentModelInfo
    id: String
    model: String
    provider: String
    api_type: String
    context_window_tokens: u64
    thinking: Option<String>
    reasoning_effort: Option<String>
    行为：Agent自有公共纯值，不包含endpoint/凭据、不持有其他领域组件或World。
AgentSyncSnapshot
    agents: Vec<AgentPublicState>
AgentTurnSnapshot
    address: String
    turn: Option<String>
AgentUpdateContext
    directory: AgentDirectory
    owner: Entity
    source_epoch: u64
    agents: SyncContext<AgentSyncSnapshot>--所有Agent克隆同一个应用级集合来源
    turn: SyncContext<AgentTurnSnapshot>
AgentChangeResult
    Unit | Turn(String)--StartTurn在同一写入中返回实际新turn，不另行读取来猜测结果
AgentChange
    Register(AgentPublicState) | Transition(AgentState) | StartTurn
    FinishTurn(String) | AbortTurn(String) | Fault(AgentError)
    Tools(Vec<AgentToolInfo>) | Sandboxes { values: Vec<AgentSandboxInfo>, selected: Option<String> }
    Model(Option<AgentModelInfo>) | Remove
AgentSyncBinding
    directory: AgentDirectory
    bind: Arc<dyn Fn(Entity, String) -> AgentUpdateContext + Send + Sync>
    行为：工厂共享Clients和集合来源，per-Agent turn使用当前实际来源租约；快照读取仅捕获Weak共享数据，避免来源索引强引用回环。

AgentStartParameters：启动参数，公开类型
    driver: String--Driver文件定位字符串
    project_root: PathBuf
    home_root: PathBuf
    database_path: PathBuf
    settings_path: PathBuf

AgentState：生命周期状态，公开枚举
    Starting--仅允许匹配当前启动操作的步骤与结果
    Idle--允许开始回合、StopAgent、RestartAgent
    Working--允许结束或中止当前回合、StopAgent、RestartAgent
    Restarting--仅允许匹配当前操作的步骤与结果
    Stopping--仅允许匹配当前操作的步骤与结果

```

# error

## 类型

```text
-- 公开
AgentError：生命周期错误，公开类型
    kind: AgentErrorKind
    message: String
    impl Display + std::error::Error

AgentErrorKind：错误类别，公开枚举
    InvalidRequest | AgentMissing | StateRejected
    StartFailed | StopFailed | RestartFailed | CleanupFailed | ReplyClosed
```

# 持有关系

```text
World
└── Agent Entity
    ├── Address（address）
    ├── Agent → AgentShared（所有变更经AgentUpdateContext）
    ├── ToolTasks（tool_core，InitToolTasks挂载）
    ├── ActiveInference（inference_core，InitInference挂载）
    ├── Driver运行组件（Lua领域core）
    ├── MclQueue（MCL领域core）
    ├── 数据库与历史组件（存储领域core）
    └── 各资源路径集合（所属领域core）

异步闭包
└── FlowContext
    ├── Entity
    ├── owned参数及事件发送能力
    └── 本步骤oneshot接收端
```
