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
        克隆当前AgentStorage供日志写入，固定Agent与本次Driver实例关联
        安装新实例和通知组件，克隆本次wake的Arc及实例关联
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
create_mcl_host(instance: DriverLuaVmInstance, events: RuntimeEventSender, cancel: CancellationToken) -> Arc<dyn LuaHostFunction>
    行为：构造本plugin的MclHost，只引用mcl_core的请求和结果契约，不调用mcl_plugin。

create_wait_host(slot: Arc<Mutex<DriverWakeSlot>>) -> Arc<dyn LuaHostFunction>
    行为：构造WaitHost，捕获本次共享槽位；调用时直接await DriverWakeSlot::wait(slot)，不查询World或发送等待事件。

start_vm(context: StartContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：等待ready；失败或超时请求cancel，发送StopDriverLuaVm并等待oneshot，返回错误及退出事实。

stop_vm(context: StopContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：等待原ended；VM退出且Lua释放后返回成功，通道丢失返回EndNotificationLost。

run_driver(context: RunContext) -> LocalFuture
    行为：
        在Workspace线程创建Lua、设置预算，按共享宿主定义安装margatroid对象
        context.kind固定driver，agent_info放入只读context，不再注入独立agent_info、mcl、wait全局
        公共能力复用同一绑定实现；margatroid.driver绑定本plugin构造的MclHost与WaitHost
        margatroid.tools与margatroid.task保留拒绝入口，不持有ToolTask能力，不伪造工具任务
        在宿主边界捕获脚本编译、加载及Driver主入口的Lua异常，保留原始错误、来源、行号和traceback
        异常通过append_agent_log写入当前Agent独立日志表；等待记录完成后沿原失败及退出流程处理
        同一异常只在入口边界记录一次；主动取消与正常退出不记录为Lua脚本故障
        日志写入失败使用现有诊断通道，不覆盖原错误，不递归log
        对外LuaRuntimeError只提供分类与简洁说明，原始诊断不自动进入会话
        释放Lua及其执行资源后完成ended并发送既有DriverLuaVmExited
        未ready即退出则通过剩余ready发送端回复启动失败；关闭wake，不关闭Workspace线程
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
            行为：将Lua参数转换为MclInstruction；绑定source为instance.owner及本次start_id，发送MclCommand并等待reply；MclValue序列化为Lua值，参数校验、reply丢失及fallback MclError统一返回MclValue::Error({type:error,message,request_id})；单次MCL取消不退出VM，只有VM取消保留Err；不执行或解析指令推断返回形状。

WaitHost：等待适配，私有类型
    slot: Arc<Mutex<DriverWakeSlot>>
    impl LuaHostFunction for WaitHost
        call(&self, arguments: Vec<LuaValue>, cancel: CancellationToken) -> HostFuture
            行为：校验无参数，等待共享槽位或取消；完成及取消时清除本次waiting标记，锁不跨await。

StartContext：启动等待上下文--instance、ready接收端、cancel、RuntimeEventSender、原reply
StopContext：停止等待上下文--instance、ended接收端、原reply
RunContext：执行上下文--instance、program、environment、wake、ready发送端存储、ended发送端、cancel、RuntimeEventSender、AgentStorage日志能力及宿主固定关联
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

Margatroid：共享Lua userdata
    context: serde_json::Value--只读执行身份、调用关联及arguments
    files: 文件操作子对象--read、write、list、info、create_directory、matches
    data: 数据转换子对象--encode、decode及null标记
    text: 文本搜索子对象--search
    process: 子进程子对象--start
    driver: Driver协调子对象--mcl、wait
    tools: 工具定义子对象--register
    task: 运行任务子对象--return_result、finish、append_output、replace_output、read_output、send、receive

HostNamespace：只读子对象
    namespace: Files | Data | Text | Process | Driver | Tools | Task
    identity: Driver | Tool--宿主固定
    phase: Configure | Execute
    methods: 已裁剪的能力回调，不向Lua暴露可修改回调表

HostDefinition：两种执行环境共用的宿主定义
    kind: HostKind
    context: serde_json::Value--真实宿主上下文
    arguments: serde_json::Value--安装时合入context.arguments，不作为Lua根对象字段

HostKind：执行身份
    Driver | Tool

LuaHostContext字段约定：JSON对象保存真实执行元数据
    kind: Driver | Tool
    arguments: LuaValue--工具调用参数，Driver为data.null
    agent_info: LuaValue--当前Agent真实信息
    project_root: Option<PathBuf>--只在当前环境确实提供时存在
    driver_instance: Option<String>
    task_id, call_id, alias: Option<String>--仅工具提供，不虚构Driver任务
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
        files、data、text、process、driver、tools、task均为不可替换子对象，未enable时方法均明确拒绝
        data.null保留空值身份，根对象不再提供version、arguments或null字段
        不保留旧全局mcl、wait、agent_info、顶层文件编解码方法或tool命名空间作为兼容入口

Margatroid::enable(object: &AnyUserData, callbacks: Table) -> Result<(), LuaError>
    行为：
        只选取公共方法及当前身份的专用能力，冻结为不向Lua公开的私有回调集合
        公共方法按files、data、text、process绑定，log与wait_any保留在根对象
        Driver协调能力只绑定driver；工具定义与任务能力分别绑定tools和task
        tools只登记定义，task只控制运行任务，不能因owner字段或命名重叠隐式联动
        拒绝重复enable；错误身份的命名空间不持有真实回调
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

margatroid.driver:mcl(arguments: Vec<LuaValue>) -> HostFuture
    行为：
        require identity == Driver
        使用现有MclHost参数及错误契约；source绑定真实Agent地址和本次Driver实例
        Tool中调用明确返回执行身份错误，不发送MCL事件

margatroid.driver:wait() -> HostFuture
    行为：
        require identity == Driver
        复用当前DriverWakeSlot等待及取消语义；Tool中调用明确失败
        Driver主入口捕获与日志不改变既有wake/ready/ended契约

HostNamespace::callback(name: &str) -> Result<Function, LuaError>
    行为：
        根据命名空间和固定执行身份查找冻结的回调，具体共享方法签名见Tool设计
        driver仅允许Driver身份，tools及task仅允许Tool身份且已确认execute阶段
        文件、数据、文本、进程和日志沿真实执行环境的权限和阶段启用
        错误身份明确失败，不创建工具任务、不伪造回复端、不扩大宿主权限

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
    ├── Margatroid共享宿主对象
    │   ├── 公共能力、日志与只读context
    │   ├── driver -> MclHost及WaitHost
    │   └── tools/task -> 无能力的拒绝入口
    ├── AgentStorage克隆句柄 -> agent_logs
    ├── MclHost（只发送既有mcl_core::MclCommand）
    ├── WaitHost（捕获与组件相同的wake Arc）
    └── ended发送端

mecs全局异步设施
├── start_vm
└── stop_vm
```
