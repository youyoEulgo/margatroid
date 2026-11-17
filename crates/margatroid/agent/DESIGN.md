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
    agent_state(&self, agent: Entity) -> Result<&AgentState, AgentError>
        行为：
            读取Agent状态，目标不存在返回错误。
    agent_transition(&mut self, agent: Entity, state: AgentState, turn: Option<String>) -> Result<(), AgentError>
        行为：取得Agent必需上下文，调用update_agent(Transition)，只允许领域入口在其已验证操作中使用。
    agent_ready(&mut self, agent: Entity) -> Result<(), AgentError>
        行为：Agent领域检查Starting或Running且无turn，再经统一入口转Running；拒绝Error/Stopping及已有turn，MCL只调用此方法。
    agent_start_turn(&mut self, agent: Entity) -> Result<String, AgentError>
        行为：
            仅Running允许；已有有效turn时直接返回同一标识，不生成或替换回合，不重复发布变化。
            turn为空时同步生成并发布新turn，返回其标识；Agent始终Running。
    agent_finish_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>
        行为：
            只验证Running、传入turn非空且与当前有效turn一致；当前turn已空时允许原调用链幂等结束。
            经统一入口清空turn，状态保持Running；不在本方法读取推理或工具组件，实际执行收口由调用方先等待；
            有活动turn时核对匹配，中止后turn为空由当前Driver调用链完成；
            调用方plugin核对实际执行来源及AgentSession，拒绝旧来源完成。
    agent_abort_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>
        行为：
            仅Running且turn匹配时同步清空turn，保持Running；
            否则返回错误。此操作必须先于同帧MCL、推理和工具调用受理；
            不在agent_core跨域取消任务，调用方随后通过各领域core执行推理取消和ToolTasks未return任务的原子强制return及清理。
    impl WorldAgentExt for World
        行为：
            只访问Agent自身共享信息；ready/start_turn/finish_turn/abort_turn经同一AgentUpdateContext调用update_agent，不能直接修改字段或旁路同步。
```

```text
-- 公开，定义于agent_plugin
AgentPlugin：生命周期插件，公开类型
    schedule: String--启停System所属计划
    sync: AgentUpdateContext
    driver_root: PathBuf--既有网络默认创建材料的解析根，安装为AgentResourceRoot
    new(sync: AgentUpdateContext) -> Self
        行为：必需发布上下文，默认schedule为UPDATE，不提供无上下文生产构造。
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
Agent：Agent领域组件，不可克隆
    name: String--私有，只读访问器name() -> &String
    workspace: Entity--私有，只读workspace() -> Entity
    state: AgentState--私有，只读state() -> AgentState
    turn: Option<String>--私有，只读turn() -> &Option<String>
    fault: Option<AgentError>--私有，只读fault() -> Option<&AgentError>
    updates: AgentUpdateContext
    source_alive: Arc<AtomicBool>
    new(name: String, workspace: Entity, state: AgentState, turn: Option<String>, updates: AgentUpdateContext) -> Self
    updates(&self) -> AgentUpdateContext
    source_lease(&self) -> Arc<AtomicBool>
    reset_source(&mut self)
        行为：旧源失效后分配新的有效标记。
    close_source(&self)
    impl Component + Debug
    impl Drop：关闭实际来源，已经排队的旧推理帧不能复活。

AgentResources：Component
    parameters: AgentStartParameters--保留实际创建参数；UI目录是引用，配置目标为Agent持久文件
AgentResourceRoot(PathBuf)：Resource
    行为：既有网络agent.start按Workspace成员默认资源解析显式配置/UI路径；Lua创建直接传两条路径，不使用此默认解析根。
AgentSession(String)：Component
    new() -> Self
    行为：每次实际Agent运行唯一，原TEMP Block命名空间沿用此id，结束时清理不落盘。


```

# events

## 类型

```text
-- 公开
StartAgent：启动请求，公开事件
    owner: Entity--实际Workspace或父Agent
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
    接收闭包结果：None
    提交闭包：等待StopAgent原oneshot的普通异步闭包，返回Result<(),AsyncTaskError>
    闭包产出事件：停止成功后一次StartAgent
    oneshot回复：停止失败直接回复原RestartAgent，成功由普通StartAgent完成原reply
    行为：仅路由RestartAgent -> handle_restart_agent，不接收独立RestartOutput或运行第二套生命周期。

start_agent_system(world: &mut World)
    处理事件：StartAgent
    接收闭包结果：Result<StartOutput, StartError>
    提交闭包：start_flow(FlowContext) -> Future<Output = Result<StartOutput, StartError>>
    闭包产出事件：InitToolTasks、InitInference
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
update_agent(world: &mut World, owner: Entity, context: &AgentUpdateContext, change: AgentChange) -> Result<(), AgentError>
    行为：
        核对实际Agent存在，Transition只在此写入state/turn；Fault在同次入口记录故障、清turn并转Error。
        成功进入Running清除旧故障；失败或可见值不变不发布。
        释放Agent可变借用后调用context.notify(Changed)，同步探针在World线程取得owned公共投影并提交共享视图，然后直接通知Clients。
        不把World捕获到异步prepare；异步网络读取的是owned版本化投影缓存。
notify_agent_runtime(world: &mut World, owner: Entity, stopped: bool) -> Result<Vec<AgentObservationWait>, AgentError>
    行为：实际安装重置推理来源租约、停止关闭来源，调用RuntimeInstalled/RuntimeStopped生命周期回调，返回旧文件/SQL监控的真实完成等待。
remove_agent(world: &mut World, owner: Entity)
    行为：真实停止收口后销毁Entity，再经同一上下文更新完整Agent目录并退休turn来源；仍有监控等待则属于未完成清理错误。
wait_observers(observers: Vec<AgentObservationWait>) -> Future<Output = Result<(), AgentError>>
    行为：并发等待所有真实监控完成，聚合失败为CleanupFailed，不以取消请求或丢Future替代。

handle_start_agent(world: &mut World, request: StartAgent)
    行为：
        校验Workspace和成员创建请求；
        同步创建Agent Entity并经update_agent(Register)登记Starting公共视图，再提交start_flow；
        拒绝时发送启动错误。

handle_stop_agent(world: &mut World, request: StopAgent)
    行为：
        通过update_agent按白名单同步转Stopping并直接发布agent.sync；
        Starting/Running/Error可受理；关闭新请求受理，再提交stop_flow，等待StartupBarrier真实完成后才清理资源；
        拒绝时发送关闭错误。

handle_restart_agent(world: &mut World, request: RestartAgent)
    行为：按workspace/name定位既有Agent，捕获实际AgentOwner和请求parameters；直接调用handle_stop_agent，停止使用普通StopAgent的状态/取消/清理规则。
        仅await真正StopAgent.reply；成功后提交普通StartAgent，携带原owner/workspace/name/parameters及原重启reply；新Agent Entity和Session由StartAgent创建。
        停止失败或回执丢失立即回复，不提交StartAgent；不检查ToolTasks是否非空、不提前校验启动参数、不额外改状态/装Barrier/重载定义、不保留旧Entity。

handle_agent_exception(world: &mut World, event: AgentException)
    行为：
        仅当前Starting/Running可报告故障，Stopping/Error不被旧异常覆盖；通过update_agent设置Error、清turn，cancel_runtime关闭MCL及取消推理/任务，直接发布agent.sync；不伪报清理完成。

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
    行为：
        核对Entity及对应生命周期状态；
        忽略失效结果。
        资源挂载成功：保留Starting，通过原StartAgent.reply回复Entity；只有后续MCL ready进入Running。
        失败：
            有完整清理证明则经update_agent(Remove)移出公共目录再销毁Entity，否则保留宿主并将匹配本次FlowError.state的运行转Error，关闭普通受理。
            通过原reply回复错误。

handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
    行为：
        核对Entity及对应生命周期状态；
        忽略失效结果。
        关闭成功：经update_agent(Remove)移出公共目录，再销毁Entity，通过原StopAgent.reply回复原Entity。
        失败：
            有完整清理证明则经update_agent(Remove)移出公共目录再销毁Entity，否则保留宿主并将匹配本次FlowError.state的运行转Error，关闭普通受理。
            通过原reply回复错误。


```

```text
-- 私有
prepare(context: &FlowContext) -> Future<Output = Result<(AgentStorage, AgentResources), AgentError>>
    行为：复用受监督blocking读取与open_storage；持久settings_path不存在时只读initial_settings_path并校验TOML，存在时不依赖或重读初值文件。
        固定state.sqlite/setting.toml路径沿原symlink检查，已有配置保留不覆盖；AgentResources保留完整parameters，UI路径只用作监控来源，不创建UI目录、不复制文件、不读取旧Agent本地UI。
install_runtime(context: &FlowContext, storage: AgentStorage, source: AgentResources) -> Future<Output = Result<(), AgentError>>
    行为：复用原World挂载入口，核对旧推理/MCL/工具已收口；挂载storage、AgentSession、AgentWaitSlot、AgentResources、MclQueue和typed资源集合。
        不创建Lua环境、VM或模型默认配置；模型由EFFECT model_set通过原typed入口安装。
        notify_agent_runtime仅建立传入UI目录/COMMIT监控并等待旧监控真实退出。
start_steps(context: &FlowContext) -> Future<Output = Result<(), StartFailure>>
    行为：prepare与install_runtime后同时提交InitToolTasks/InitInference并收齐原oneshot；失败不提前丢弃另一项，不启动VM。
stop_runtime(context: &FlowContext, _require_vm: bool) -> Future<Output = Result<(), AgentError>>
    行为：关闭Agent受理、等待槽、推理及工具；按AgentOwner精确找到直接子Agent并通过原StopAgent逐个等待其递归清理。
        复用原tasks.finish、inference.wait_finished及MclDrain真实等待；之后按AgentSession调用DropTemporaryBlocks，保留持久Block和历史。
        移除AgentResources并等待实际UI/记录监控收口；不读取或终止创建者VM，不销毁协调Entity。

```

# types

## 类型

```text
-- 私有
StartupCompletion：私有共享启动完成证明
    done: AtomicBool
    changed: tokio::sync::Notify
    finish(&self)：Release发布真实启动Future完成并notify_waiters
    wait(&self)：先注册通知后检查done，避免丢失唤醒
StartupBarrier(Arc<StartupCompletion>)：私有Component
    行为：仅StartAgent创建时安装；guarded_start在含panic收口后finish；stop_flow先wait，不能先销毁仍被初始化worker使用的宿主。
FlowContext：闭包上下文
    id: String--原请求ID
    agent: Entity
    events: RuntimeEventSender
    parameters: Option<AgentStartParameters>--启动为Some，关闭为None
    reply: Option<oneshot::Sender<Result<Entity, AgentError>>>--原请求回复端
    startup: Arc<StartupCompletion>--同一实际启动工作完成屏障

StartOutput / StopOutput：对应闭包的成功载荷
    id: String--原请求ID
    agent: Entity

StartError / StopError：对应闭包的失败载荷
    id: String--原请求ID
    agent: Entity
    error: AgentError
    cleanup_completed: bool
    state: Option<AgentState>--产生失败的实际阶段，旧阶段回调不能覆盖新操作

```

# types pub

## 类型

```text

AgentOwner(Entity)：Component--指向实际Workspace或父Agent
AgentStartParameters
    initial_settings_path: PathBuf--初始化配置文件，非运行配置位置
    ui_path: PathBuf--直接引用的UI目录
    project_root: PathBuf
    home_root: PathBuf
    database_path: PathBuf--Agent持久数据库
    settings_path: PathBuf--Agent持久配置
    行为：显式设置配置/UI来源材料、项目/存储路径；不读取driver.toml或启动VM。
Agent
    行为：AgentOwner为独立组件，不在Agent中持有VM。
    行为：归属决定持久路径及清理树，workspace始终表示根Workspace；不持DriverLuaVm或VM生命周期责任。
StartAgent
    owner: Entity
    行为：仅创建Entity、复用prepare/open_storage/install_runtime及InitToolTasks/InitInference，资源安装完成返回实际Entity；ready由明确目标MCL推进。

-- 公开
AgentObservationWait = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send>>
AgentUpdateKind
    Changed | RuntimeInstalled | RuntimeStopped | Removed
AgentUpdateContext：必需typed同步发布能力，Resource + Clone
    publish: Arc<dyn Fn(&mut World, Entity, AgentUpdateKind) -> Vec<AgentObservationWait> + Send + Sync>
    public: Arc<dyn Fn(Entity, AgentPublicUpdate) + Send + Sync>
    scope: Arc<AtomicBool>
    scopes: Arc<Mutex<BTreeMap<Entity, Arc<AtomicBool>>>>
    new(publish, public) -> Self
    notify(&self, world: &mut World, owner: Entity, kind: AgentUpdateKind) -> Vec<AgentObservationWait>
        行为：RuntimeStopped/Removed关闭旧资源scope，再调用所属生命周期探针；其他实际状态变化不会创建调查任务。
    scoped(&self, owner: Entity) -> Self
        行为：按实际来源衍生资源回调租约，停止/重启后旧回调失效。
    update_public(&self, owner: Entity, value: AgentPublicUpdate)
        行为：当前scope有效才提交安全公共纯值，DTO回调只更新共享投影，不访问World。
AgentChange
    Transition { state: AgentState, turn: Option<String> } | Fault(AgentError)
AgentPublicUpdate
    Tools(Vec<PublicToolInfo>) | Sandboxes(Vec<(String, PathBuf)>)
    Sandbox(Option<String>) | Model(Option<PublicModelInfo>)
PublicToolInfo
    alias: String
    path: PathBuf
    description: String
    input_schema: Value
    owner_task_id: Option<String>
    binding: Option<Value>
PublicModelInfo
    id: String
    model: String
    provider: String
    api_type: String
    context_window_tokens: u64
    thinking: Option<String>
    reasoning_effort: Option<String>
    行为：只含安全公共字段，不含endpoint/凭据。

AgentStartParameters：启动参数，公开类型
    initial_settings_path: PathBuf
    ui_path: PathBuf--显式配置/UI初值资源定位
    project_root: PathBuf
    home_root: PathBuf
    database_path: PathBuf
    settings_path: PathBuf

AgentState：生命周期状态，公开枚举
    Starting--创建/挂载/初始化；允许MCL，不允许新推理或工具执行
    Running--ready完成；普通MCL可用；推理/工具仅在请求明确携带turn时检查有效关联，自由调用不要求当前turn
    Error--明确异常；只允许查询诊断及关闭清理，不接受普通MCL
    Stopping--真实停止监督正在执行，清理完成才移除Entity
    行为：不存在Idle、Working或Restarting；turn为独立Option<String>，回合开始、中止和结束不修改Running。


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
    ├── Agent（私有状态，所有变更经AgentUpdateContext）
    ├── ToolTasks（tool_core，InitToolTasks挂载）
    ├── ActiveInference（inference_core，InitInference挂载）
    ├── AgentSession与AgentWaitSlot（不拥有执行者）
    ├── MclQueue（MCL领域core）
    ├── 数据库与历史组件（存储领域core）
    └── 各资源路径集合（所属领域core）

异步闭包
└── FlowContext
    ├── Entity
    ├── owned参数及事件发送能力
    └── 本步骤oneshot接收端
```
