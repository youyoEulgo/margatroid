# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System区分事件、闭包结果、提交闭包和oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib        lua_runtime/core/src/lib.rs；lua_runtime/plugin/src/lib.rs  VM通知扩展；Plugin安装
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
    行为：认领reply，检查owner、workspace及预算；拒绝已有VM；安装新实例和通知组件；克隆本次wake的Arc及实例关联；从workspace_core取得提交入口，提交run_driver工厂及start_vm闭包；提交失败撤销本次安装并回复错误。

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
    行为：核对实例，通过原reply回复；失败且无退出证明时保留组件。

handle_stop_driver_lua_vm(world: &mut World, request: StopDriverLuaVm)
    行为：认领reply，核对实例并取走原ended；请求cancel、关闭wake并提交stop_vm；重复等待返回StopAlreadyWaiting。

handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
    行为：核对实例；成功移除本次VM组件；通过原reply回复；失败保留控制能力；不销毁owner或Workspace。

handle_driver_lua_vm_exited(world: &mut World, event: DriverLuaVmExited)
    行为：核对实例并报告执行错误，不修改Agent状态，不消费ended，不替代启停回复。
```

私有：
```text
create_mcl_host(instance: DriverLuaVmInstance, events: RuntimeEventSender, cancel: CancellationToken) -> Arc<dyn LuaHostFunction>
    行为：构造本plugin的MclHost，只引用mcl_core的请求和结果契约，不调用mcl_plugin。

create_wait_host(slot: Arc<Mutex<DriverWakeSlot>>) -> Arc<dyn LuaHostFunction>
    行为：构造WaitHost，捕获本次共享槽位；调用时直接await DriverWakeSlot::wait(slot)，不查询World或发送等待事件。

start_vm(context: StartContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：等待ready；失败或超时请求cancel，发送StopDriverLuaVm并等待oneshot，返回错误及退出事实。

stop_vm(context: StopContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：等待原ended；VM退出且Lua释放后返回成功，通道丢失返回EndNotificationLost。

run_driver(context: RunContext) -> LocalFuture
    行为：在Workspace线程创建Lua、设置预算、安装请求中的agent_info等环境；在Lua Runtime内构造mcl和wait宿主绑定并exec_async；捕获返回、错误、panic及取消；释放Lua后完成ended并发送DriverLuaVmExited；未ready即退出则通过剩余ready发送端回复启动失败；关闭wake，不关闭Workspace线程。
```

# types

## 类型

私有：
```text
MclHost：MCL事件发送适配，私有类型
    instance: DriverLuaVmInstance
    events: RuntimeEventSender
    cancel: CancellationToken
    impl LuaHostFunction for MclHost
        call(&self, arguments: Vec<LuaValue>, cancel: CancellationToken) -> HostFuture
            行为：将Lua参数转换为MclInstruction；绑定source为instance.owner及本次start_id，发送MclCommand并等待reply；将MclValue或MclError转换为Lua返回值，不执行指令。

WaitHost：等待适配，私有类型
    slot: Arc<Mutex<DriverWakeSlot>>
    impl LuaHostFunction for WaitHost
        call(&self, arguments: Vec<LuaValue>, cancel: CancellationToken) -> HostFuture
            行为：校验无参数，等待共享槽位或取消；完成及取消时清除本次waiting标记，锁不跨await。

StartContext：启动等待上下文--instance、ready接收端、cancel、RuntimeEventSender、原reply
StopContext：停止等待上下文--instance、ended接收端、原reply
RunContext：执行上下文--instance、program、environment、wake、ready发送端存储、ended发送端、cancel、RuntimeEventSender
StartOutput：启动成功载荷--instance、原reply
StartError：启动失败载荷--instance、原reply、LuaRuntimeError、cleanup_completed
StopOutput：停止成功载荷--instance、原reply
StopError：停止失败载荷--instance、原reply、LuaRuntimeError
```

# types pub

## 类型

公开：
```text
DriverLuaVmInstance：VM实例
    owner: Entity
    start_id: String

LuaProgram：本次程序
    source: String
    origin: PathBuf
    libraries: LuaStandardLibraries

LuaStandardLibraries：标准库选择，公开枚举
    Minimal | Safe | Full

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
    ├── MclHost（只发送mcl_core::MclCommand）
    ├── WaitHost（捕获与组件相同的wake Arc）
    └── ended发送端

mecs全局异步设施
├── start_vm
└── stop_vm
```
