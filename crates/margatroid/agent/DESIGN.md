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
    sync: AgentUpdateContext
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
update_agent(world: &mut World, owner: Entity, context: &AgentUpdateContext, change: AgentChange) -> Result<(), AgentError>
    行为：
        核对实际Agent存在，Transition只在此写入state/turn；Fault在同次入口记录故障、清turn并转Stopping。
        成功转Idle清除旧故障；失败或可见值不变不发布。
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
        完成Driver读取后，通过notify_agent_runtime的实际来源安装回调绑定UI观察上下文，使用Driver实际清单路径启动文件监控；不等下一帧扫描组件。
        AgentStorage初始化后，通过实际资源安装回调启动该实际存储的持续提交监控。
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
    ├── Agent（私有状态，所有变更经AgentUpdateContext）
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
