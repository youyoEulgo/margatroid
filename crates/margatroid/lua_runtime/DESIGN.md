# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System区分事件、闭包结果、提交闭包和oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib        lua_runtime/core/src/lib.rs；lua_runtime/plugin/src/lib.rs  VM通知扩展；共享宿主绑定；Plugin安装
# components lua_runtime/core/src/components.rs                        DriverLuaVm组件
# events     lua_runtime/core/src/events.rs                            VM启停请求及退出通知
# system     lua_runtime/plugin/src/system.rs                          请求及闭包结果路由
# handler    lua_runtime/plugin/src/handler.rs                         VM提交、宿主注入、等待和回复
# types      lua_runtime/plugin/src/types.rs                           闭包上下文及结果
# types pub  lua_runtime/core/src/types.rs                             程序、环境及通知能力
# error      lua_runtime/core/src/error.rs                             LuaRuntimeError
```

# lib

## 类型

公开，定义于lua_runtime_core：
```text
WorldLuaRuntimeExt：VM通知扩展，公开trait
    driver_ready(&self, instance: &DriverLuaVmInstance) -> Result<(), LuaRuntimeError>
        行为：核对实例，取出ready发送端并发送Ok(())；已取走返回AlreadyReady。
    driver_wake(&self, instance: &DriverLuaVmInstance) -> Result<(), LuaRuntimeError>
        行为：核对实例，唤醒wake槽位。
    impl WorldLuaRuntimeExt for World
        行为：只访问DriverLuaVm组件。
```

公开，定义于lua_runtime_plugin：
```text
LuaRuntimePlugin：VM执行插件
    schedule: String
    config: LuaRuntimeConfig
    impl Plugin for LuaRuntimePlugin
        build(self, app: &mut App)
            行为：检查运行设施及预算，拒绝重复安装；依次安装stop_driver_lua_vm_system、start_driver_lua_vm_system、driver_lua_vm_exit_system。
```

# components

## 类型

公开：
```text
DriverLuaVm：当前VM能力，公开组件
    instance: DriverLuaVmInstance
    wake: Arc<Mutex<DriverWakeSlot>>
    ready: Arc<Mutex<Option<oneshot::Sender<Result<(), LuaRuntimeError>>>>>
    ended: Option<oneshot::Receiver<Result<(), LuaRuntimeError>>>
    cancel: CancellationToken
    impl Component for DriverLuaVm
```

# events

## 类型

公开：
```text
StartDriverLuaVm：启动请求，公开事件
    owner: Entity
    workspace: Entity
    program: LuaProgram
    environment: LuaEnvironment
    reply: Arc<Mutex<Option<oneshot::Sender<Result<DriverLuaVmInstance, LuaRuntimeError>>>>>
    impl Event for StartDriverLuaVm

StopDriverLuaVm：停止请求，公开事件
    instance: DriverLuaVmInstance
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), LuaRuntimeError>>>>>
    impl Event for StopDriverLuaVm

DriverLuaVmExited：退出通知，公开事件
    instance: DriverLuaVmInstance
    result: Result<(), LuaRuntimeError>
    impl Event for DriverLuaVmExited
```

# system

## 函数

crate公开：
```text
stop_driver_lua_vm_system(world: &mut World)
    处理事件：StopDriverLuaVm
    接收闭包结果：Result<StopOutput, StopError>
    提交闭包：stop_vm(StopContext) -> Future<Output = Result<StopOutput, StopError>>
    产出事件：None
    oneshot回复：Result<(), LuaRuntimeError>
    行为：StopDriverLuaVm -> handle_stop_driver_lua_vm；Result<StopOutput, StopError> -> handle_stop_result。

start_driver_lua_vm_system(world: &mut World)
    处理事件：StartDriverLuaVm
    接收闭包结果：Result<StartOutput, StartError>
    提交闭包：start_vm(StartContext) -> Future<Output = Result<StartOutput, StartError>>
    提交本地任务：run_driver(RunContext) -> LocalFuture
    产出事件：None
    闭包产出事件：StopDriverLuaVm
    本地任务产出事件：DriverLuaVmExited
    oneshot回复：Result<DriverLuaVmInstance, LuaRuntimeError>
    行为：StartDriverLuaVm -> handle_start_driver_lua_vm；Result<StartOutput, StartError> -> handle_start_result。

driver_lua_vm_exit_system(world: &mut World)
    处理事件：DriverLuaVmExited
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    oneshot回复：None
    行为：DriverLuaVmExited -> handle_driver_lua_vm_exited。
```

# handler

## 函数

crate公开：
```text
handle_start_driver_lua_vm(world: &mut World, request: StartDriverLuaVm)
    行为：
        认领reply，检查owner、workspace及预算，拒绝已有VM
        owner为实际执行实体，WorkspaceExecutor仍是执行线程入口；AgentStorage仅为可选日志能力，不要求已有Agent。
        安装新实例和通知组件，复用ToolTaskHandle创建LuaExecutionTask；配置有TaskSyncBinding时为Workspace内部任务派生execution_only，不投Agent专属网络同步。
        从workspace_core取得提交入口，提交run_driver工厂及start_vm闭包
        提交失败撤销本次安装并回复错误，不修改启动事件字段

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
    行为：核对实例，通过原reply回复；失败且无退出证明时保留组件。

handle_stop_driver_lua_vm(world: &mut World, request: StopDriverLuaVm)
    行为：认领reply，核对实例并取走原ended；请求cancel、关闭wake并提交stop_vm；重复等待返回StopAlreadyWaiting。

handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
    行为：核对实例；成功移除本次VM组件；通过原reply回复；失败保留控制能力；不销毁owner或Workspace。

handle_driver_lua_vm_exited(world: &mut World, event: DriverLuaVmExited)
    行为：
        核对实例并报告已完成原始诊断记录的简洁执行错误
        不重复记录入口已捕获的同一Lua异常
        不修改Agent状态，不消费ended，不替代启停回复
```

私有：
```text
start_vm(context: StartContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：等待ready；失败或超时请求cancel，发送StopDriverLuaVm并等待oneshot，返回错误及退出事实。

stop_vm(context: StopContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：等待原ended；VM退出且Lua释放后返回成功，通道丢失返回EndNotificationLost。

run_driver(context: RunContext) -> LocalFuture
    行为：沿原Workspace线程和监督运行execute_lua，捕获诊断；有AgentStorage时按原append_agent_log落库，无目标存储时通过运行日志记录。
        执行完成后等待process registry真实cleanup，再关闭实际任务scope，经World complete_cleanup移除任务；不改变或销毁任何Agent。
        进程真实清理后通过LuaServices.execution_finished交付本次执行诊断，等待绑定Agent日志提交及故障交接，再完成ended及DriverLuaVmExited；尚未交接启动则短任务正常结束确认成功、失败确认原错误。
execute_lua(context, environment, logger, processes) -> Future<Output = Result<(), LuaRuntimeError>>
    行为：调用共享create_environment，安装取消hook后复用configuration_globals/load_program；configure返回ToolConfiguration，纯阶段能力未启用。
        enable_execution加载同一标准库并恢复保存全局，Margatroid.enable安装相同公共方法；读取execute并异步调用，配置超时同时约束CPU及等待。
        source身份为执行Entity/实例，目标由方法显式指定；context包含workspace/name/resource_dir/task_id，不预造agent_info。
        公共服务回调通过LuaServices，文件/data/text/process沿既有共享安装器；全程不按HostKind裁剪API。

```

# types

## 类型

私有：
```text
StartContext：启动等待上下文--instance、ready接收端、cancel、RuntimeEventSender、原reply
StopContext：停止等待上下文--instance、ended接收端、原reply
DriverLogger：私有诊断上下文--可选storage、LuaServices、真实LuaExecutionIdentity、World发送能力、origin及待捕获诊断
    append：已有storage沿原blocking提交；无storage的主动log通过execution_log提交已绑定Agent；无Agent或未处理故障通过RunContext.runtime_logs提交完整应用报告，不能只打印控制台；故障另由execution_finished逐绑定完成Agent报告。
RunContext.runtime_logs: Option<RuntimeLogStore>--Compose中必需安装；独立运行夹具可使用AgentStorage
RunContext：执行上下文--instance、program、environment、wake、ready发送端存储、ended发送端、cancel、RuntimeEventSender、可选AgentStorage、workspace、LuaServices及宿主固定关联
StartOutput：启动成功载荷--instance、原reply
StartError：启动失败载荷--instance、原reply、LuaRuntimeError、cleanup_completed
StopOutput：停止成功载荷--instance、原reply
StopError：停止失败载荷--instance、原reply、LuaRuntimeError
```

# types pub

## 类型

公开：
```text

LuaExecutionIdentity
    source: Entity--实际Lua执行实体，独立于任意目标Agent
    workspace: Entity
    address: String
    instance: String
    cancel: CancellationToken
LuaServices：Resource + Clone
    invoke: Arc<dyn Fn(LuaExecutionIdentity, String, Value) -> LuaServiceFuture + Send + Sync>
    行为：公共宿主服务边界，由应用宿主绑定既有World/event能力；工具worker经已有双向管道转发，同源执行环境不按Driver/Tool裁剪方法。
create_environment(definition: HostDefinition, memory_limit: usize) -> mlua::Result<(Lua, AnyUserData)>
    行为：统一TABLE/STRING/MATH/UTF8标准库、Lua allocator预算、readonly Margatroid安装及configure阶段隐藏项。
configuration_globals(lua: &Lua) -> mlua::Result<Vec<(&'static str, Value)>>
    行为：保存并隐藏pcall/xpcall/load/setmetatable/dofile/loadfile/warn，Driver与工具同源加载配置阶段。
enable_execution(lua: &Lua, saved: Vec<(&'static str, Value)>) -> mlua::Result<()>
    行为：两种启动方式共同加载IO/OS/COROUTINE、恢复保存全局并提供package.config。
load_program(lua: &Lua, source: &str, origin: &Path) -> mlua::Result<()>
    行为：同一来源命名规则加载程序一次，供configure/execute两阶段使用。
LuaAgentMonitor：Component（不持VM所有权）
    unlink: Option<Box<dyn FnOnce() + Send + Sync>>
    new(unlink) -> Self
    impl Drop：取出并调用精确解绑动作，Agent旧会话不再接收执行故障；不等待、不取消源VM。
AgentWaitSlot(Arc<Mutex<DriverWakeSlot>>)：Component + Default + Debug，不可Clone
    impl Drop：关闭槽位，即使等待者还持Arc也能结束等待。
    行为：wait(agent)登记一次唤醒，wake取出发送；取消/关闭解除并结束等待，保留未消费唤醒避免注册竞态。

DriverLuaVmInstance：VM实例
    owner: Entity
    start_id: String

LuaProgram：本次程序
    source: String
    origin: PathBuf

LuaEnvironment：注入环境
    globals: BTreeMap<String, LuaBinding>
    modules: BTreeMap<String, LuaBinding>

LuaBinding：绑定，公开枚举
    Value(LuaValue)
    Function(Arc<dyn LuaHostFunction>)

LuaValue：宿主值，公开枚举
    Nil | Boolean(bool) | Integer(i64) | Number(f64) | String(String)
    Array(Vec<LuaValue>) | Object(BTreeMap<String, LuaValue>)

LuaHostFunction：异步宿主接口，公开trait
    call(&self, arguments: Vec<LuaValue>, cancel: CancellationToken) -> HostFuture
        行为：返回异步结果，不借用World。

Margatroid：共享Lua userdata
    context: serde_json::Value--只读执行身份、调用关联及arguments
    files: 文件操作子对象--read、write、list、info、create_directory、matches
    data: 数据转换子对象--encode、decode及null标记
    text: 文本搜索子对象--search
    process: 子进程子对象--start
    mcl: execute--明确Agent目标
    agent: create--无需Agent MCL队列
    workspace: agents--实时成员
    wait(agent_address: String)--根对象异步方法
    tools: 工具定义子对象--register
    task: 运行任务子对象--return_result、finish、append_output、replace_output、read_output、send、receive

HostNamespace：只读子对象
    namespace: Files | Data | Text | Process | Tools | Task | Workspace | Mcl | Agent
    callbacks: Arc<Mutex<Option<BTreeMap<String, Function>>>>--execute阶段冻结，不暴露可写表

HostDefinition：两种执行环境共用的宿主定义
    kind: HostKind
    context: serde_json::Value--真实宿主上下文
    arguments: serde_json::Value--安装时合入context.arguments，不作为Lua根对象字段

HostKind：执行身份
    Driver | Tool

LuaHostContext字段约定：JSON对象保存真实执行元数据
    kind: Driver | Tool
    arguments: LuaValue--工具调用参数，Driver为data.null
    agent_info: Option<LuaValue>--按需工具的调用方Agent信息；自动执行尚未创建Agent时缺省
    project_root: Option<PathBuf>--只在当前环境确实提供时存在
    driver_instance: Option<String>
    task_id: String--实际执行任务id
    call_id, alias: Option<String>--有模型调用时提供
    owner_task_id: Option<String>
    binding: Option<LuaValue>
    resource_dir, temp_dir: Option<PathBuf>

LuaFailureDiagnostic：从类型化Lua错误提取的诊断
    class: LuaFailureClass
    message: String--经过分类的可展示说明
    traceback: Option<String>
    capture(error: &LuaError) -> Self
        行为：
            递归解开Callback、WithContext和嵌套External错误，保存可用traceback
            仅对Lua Runtime或Syntax来源位置进行清理，不改写任意业务或成功文本
    capture_with(error: &LuaError, external_message: Fn(&dyn Error) -> Option<String>) -> Self
        行为：
            允许领域适配器从真实类型化ExternalError取得简洁说明
            调用方在提取前保存error.to_string()作为raw_error，stage和origin由执行上下文补齐

LuaFailureClass：错误分类
    Syntax | Runtime | Callback | External | Memory | Other

Margatroid::install(lua: &Lua, definition: HostDefinition) -> Result<AnyUserData, LuaError>
    行为：
        两种运行时复用同一userdata、字段getter和值转换，不在plugin间互相调用
        强制context.kind匹配宿主身份，context及其arguments返回独立副本
        files、data、text、process、mcl、agent、workspace、tools、task均为不可替换子对象，未enable时方法均明确拒绝
        data.null保留空值身份，根对象不再提供version、arguments或null字段
        不保留旧全局mcl、wait、agent_info、顶层文件编解码方法或tool命名空间作为兼容入口

Margatroid::enable(object: &AnyUserData, callbacks: Table) -> Result<(), LuaError>
    行为：
        选取统一公共方法，冻结为不向Lua公开的私有回调集合
        公共方法按files、data、text、process绑定，log、wait和wait_any保留在根对象
        mcl/agent/workspace与tools/task对所有Lua执行者同等开放，缺失实际目标或任务能力明确失败
        tools只登记定义，task只控制运行任务，不能因owner字段或命名重叠隐式联动
        拒绝重复enable；HostKind只用于来源元数据，不决定能力集合
        每次调用均核对固定身份，不从Lua可写字段推断权限
        无能力或错误身份时返回明确宿主调用错误，不返回nil索引错误
        异步方法先克隆执行句柄并释放userdata借用，再await

margatroid:log(level: String, message: String, details: Option<LuaValue>) -> Future<Output = ()>
    行为：
        校验等级和可表达的附加数据；通过日志回调补齐当前Agent、执行身份、来源、实例和可用关联
        Driver回合关联在调用时由宿主取得，空闲时为None，不把启动时的turn缓存为全VM关联
        Tool回合与调用关联取已受理请求的固定材料
        Driver回调使用owned AgentStorage并spawn_blocking调用append_agent_log，等待真实写入完成
        Tool回调通过私有worker协议请求宿主写入同一张agent_logs并等待确认
        共享绑定只调用传入回调，不引用storage_core或私有工具协议类型
        脚本不能指定或覆盖可信关联；不追加历史、不触发推理、不结束回合
        失败返回写入错误，不递归log；工具configure阶段不开放该IO方法

margatroid.mcl:execute(target: String, instruction: String, arguments: Option<Vec<Value>>) -> Future<Value>
    行为：公共服务固定真实source身份，以明确目标提交原MclCommand，原oneshot/审计/取消保留；nil参数等价省略，目标仅Starting/Running可用。
        成功始终返回结果对象，Unit返回空JSON对象{}，在Lua中保留object标记；MCL执行失败返回Error对象，可由Lua检查后继续。
margatroid.agent:stop(target: String) -> Future<Value>
    行为：同工具bin公共注入服务，不走MCL，提交原StopAgent并外部监督，不拥有或终止创建者VM。
margatroid.agent:create({owner: String, name: String, config_path: String, ui_path: String}) -> Future<Value>
    行为：无需既有MCL队列，公共服务发送StartAgent并等待资源安装返回，Agent归属与Lua执行生命周期独立；config_path只初始化持久配置，ui_path只保存引用并直接监控，均不从执行程序路径推导。
margatroid:wait(agent_address: String) -> Future<()>
    行为：公共服务在目标AgentWaitSlot上登记一次等待，复用DriverWakeSlot防丢失唤醒及取消guard；初次等待交接自动执行启动，Agent.ready不拥有VM。
HostNamespace::callback(name: &str) -> Result<Function, LuaError>
    行为：
        根据命名空间和固定执行身份查找冻结的回调，具体共享方法签名见Tool设计
        所有公共namespace共享可用方法，执行阶段确认后按宿主能力调用
        文件、数据、文本、进程和日志沿真实执行环境的权限和阶段启用
        缺失能力或错误namespace明确失败，TaskHandle和取消归属由宿主真实注入

HostFuture：公开类型别名
    Pin<Box<dyn Future<Output = Result<LuaValue, LuaRuntimeError>> + Send>>

DriverWakeSlot：等待槽位
    sender: Option<oneshot::Sender<()>>
    pending_wake: bool
    waiting: bool
    closed: bool
    wait(slot: Arc<Mutex<Self>>) -> Future<Output = Result<(), LuaRuntimeError>>
        行为：短锁消费提前wake或建立等待；await前释放锁；拒绝并发wait。
    wake(&mut self) -> Result<(), LuaRuntimeError>
        行为：有sender则通知，否则锁存一次wake。
    close(&mut self)
        行为：关闭槽位并释放sender。

LuaRuntimeConfig：执行预算
    max_source_bytes: usize
    max_memory_bytes: usize
    instruction_check_interval: u32
    startup_timeout: Option<Duration>
NamespaceKind::Workspace
    name: "workspace"
    agents()--异步，只在执行能力安装后提供，工具端经RunnerFrame::WorkspaceAgents请求实时成员
margatroid.context.workspace
    address: String
    name: String
    project_root: PathBuf
    行为：运行环境注入所属Workspace固定身份；不注入会过期的成员快照，不暴露Entity或VM实例。
工具task.read_output/send/finish
    目标：TaskReference { agent: Option<String>, task_id: String }
    行为：字符串仍作为当前Agent任务id的便捷输入；结构化目标支持同Workspace其他Agent，后端每次校验实际实例。


```

# error

## 类型

公开：
```text
LuaRuntimeError：VM错误
    kind: LuaRuntimeErrorKind
    description: String
    impl Display + std::error::Error

LuaRuntimeErrorKind：错误类别，公开枚举
    InvalidRequest | InstanceNotFound | InstanceMismatch | AlreadyReady
    SourceTooLarge | EnvironmentFailed | CompileFailed | ExecutionFailed
    StartupTimeout | Cancelled | CleanupFailed | StopAlreadyWaiting
    EndNotificationLost | AlreadyWaiting | Closed

```

# 持有关系

```text
Agent Entity
├── Agent（agent_core）
└── DriverLuaVm（lua_runtime_core）
    ├── instance
    ├── wake及ready
    ├── ended接收端
    └── cancel

Workspace专属线程
└── run_driver
    ├── Lua与LocalFuture
    ├── Margatroid共享宿主对象
    │   ├── 公共能力、日志与只读context
    │   ├── mcl/agent/workspace/wait -> LuaServices
    │   └── tools/task -> 实际LuaExecutionTask与原工具任务能力
    ├── AgentStorage克隆句柄 -> agent_logs
    ├── LuaServices（复用原MCL/Agent事件与oneshot）
    ├── AgentWaitSlot（一次等待，不拥有VM）
    └── ended发送端

mecs全局异步设施
├── start_vm
└── stop_vm
```
