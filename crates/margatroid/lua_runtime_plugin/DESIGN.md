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

# lib

lib 放 Plugin、安装标记 Resource 和公开 re-export。

## 类型

公开：
```text
LuaVmId：虚拟机标识，公开元组结构体--由margatroid_types重新导出
    0: u64--运行时内递增标识

LuaRuntimePlugin：Lua运行时插件，公开单元结构体--统一创建、调度、执行和销毁Lua虚拟机，不包含任何业务领域语义
    schedule: String--System所属Schedule，私有
    config: LuaRuntimeConfig--运行时配置，私有
    new() -> Self
        构造插件：公开关联函数，默认UPDATE和LuaRuntimeConfig::default()
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开构建方法
    with_config(mut self, config: LuaRuntimeConfig) -> Self
        设置配置：公开构建方法
    impl Default for LuaRuntimePlugin
        Default：公开trait实现，调用new
    impl Plugin for LuaRuntimePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                要求RuntimePlugin和AsyncRuntimePlugin已安装
                重复安装时panic
                Schedule不存在时panic
                插入LuaRuntimeHandle、LuaRuntimeConfig、LuaRuntimeState和LuaRuntimePluginInstalled
                在schedule依次挂载lua_runtime_request_system、lua_runtime_cancel_system、lua_vm_message_system、lua_vm_receive_system和lua_runtime_result_system

LuaRuntimePluginInstalled：LuaRuntimePlugin安装标记，公开单元Resource--阻止重复安装
    impl Resource for LuaRuntimePluginInstalled
```

# system

system 放 System 函数。System 只读取本帧事件并克隆，然后调用 handler。

## 函数

crate公开：
```text
lua_runtime_request_system(world: &mut World)
    接收执行请求：crate公开System
    处理事件：LuaRuntimeRequest
    行为：克隆本帧全部请求，逐个调用handle_lua_runtime_request

lua_runtime_cancel_system(world: &mut World)
    接收取消请求：crate公开System
    处理事件：LuaRuntimeCancelRequest
    行为：克隆本帧全部请求，调用handle_lua_runtime_cancels

lua_vm_message_system(world: &mut World)
    投递长期VM消息：crate公开System
    处理事件：LuaVmMessage
    行为：克隆本帧全部消息，调用handle_lua_vm_messages

lua_vm_receive_system(world: &mut World)
    长期VM邮箱读取：crate公开System
    处理事件：LuaVmMessageReceiveRequest
    行为：克隆本帧全部请求，调用handle_lua_vm_receives

lua_runtime_result_system(world: &mut World)
    收集运行时结果：crate公开System
    处理事件：LuaRuntimeTaskFinished
    行为：克隆本帧全部完成事件，调用handle_lua_runtime_finished
```

# handler

handler 放处理函数。请求校验、VM执行、取消和邮箱配对逻辑都在此展开。

## 函数

crate公开：
```text
handle_lua_runtime_request(world: &mut World, request: LuaRuntimeRequest)
    处理执行请求：crate公开函数
    行为：
        校验request_id、重复请求和源码大小
        分配LuaVmId和CancellationToken并写入LuaRuntimeState
        LongRunning请求立即发送LuaVmStarted
        读取配置后将VM执行任务提交到独立线程
        任务结束时通过reply恰好发送一次结果，并发送LuaRuntimeTaskFinished
        校验失败时立即fail回执并发送vm_id为空的LuaRuntimeTaskFinished

handle_lua_runtime_cancels(world: &mut World, requests: Vec<LuaRuntimeCancelRequest>)
    处理取消请求：crate公开函数
    行为：
        按request_id标记取消令牌
        同时按owner_id匹配所有相关请求并取消

handle_lua_vm_messages(world: &mut World, messages: Vec<LuaVmMessage>)
    处理长期VM消息：crate公开函数
    行为：
        有pending receive时直接配对并发送LuaVmMessageReceived
        否则把value追加到VM FIFO邮箱
        不执行Lua代码

handle_lua_vm_receives(world: &mut World, requests: Vec<LuaVmMessageReceiveRequest>)
    处理邮箱读取：crate公开函数
    行为：
        邮箱非空时取出最早值并发送LuaVmMessageReceived
        邮箱为空且VM存活时保存pending receive；已有pending receive时发送InvalidRequest响应
        VM不存在时发送VmExecutionFailed响应

handle_lua_runtime_finished(world: &mut World, finished: Vec<LuaRuntimeTaskFinished>)
    处理任务完成：crate公开函数
    行为：
        清理requests、owners、sessions、mailboxes和receives
        对仍挂起的receive发送失败响应
        不将结果转换为业务领域事件
```

私有：
```text
execute_lua(program: LuaProgram, context: LuaEnvironmentContext, providers: Vec<String>, registry: Option<Arc<RwLock<LuaEnvironmentRegistry>>>, cancellation: CancellationToken) -> Result<LuaValue, LuaRuntimeError>
    执行VM：私有函数
    行为：
        按LuaStandardLibraries创建独立Lua VM
        安装环境提供器生成的全局值、宿主函数和模块
        编译并执行源码或入口函数
        宿主函数调用按阻塞函数语义暂停当前VM，等待宿主Future完成后恢复
        在宿主调用前后和指令检查点响应取消
        返回值转换为LuaValue

lua_value_size(value: &LuaValue) -> usize
    计算LuaValue大小：私有函数

to_ml_value(lua: &Lua, value: LuaValue) -> Result<MlValue, LuaRuntimeError>
    LuaValue转mlua值：私有函数

from_ml_value(value: MlValue) -> Result<LuaValue, LuaRuntimeError>
    mlua值转LuaValue：私有函数
```

# events

events 放事件类型。

## 类型

公开：
```text
LuaRuntimeRequest：Lua执行请求，公开事件--业务插件提交一个待执行的Lua程序
    request_id: String--调用方用于匹配结果的请求标识
    owner: LuaVmOwner--执行所属者和生命周期归属
    program: LuaProgram--待执行程序
    context: LuaEnvironmentContext--环境提供器使用的上下文
    providers: Vec<String>--本次执行显式启用的环境提供器名称，按该顺序安装
    scheduler: LuaScheduler--本次执行使用的调度模式
    deadline: Option<Instant>--绝对截止时间；空时由调度模式决定是否使用默认时限
    reply: LuaRuntimeReply--可从只读事件中安全取出的恰好一次回执
    impl Event for LuaRuntimeRequest

LuaRuntimeCancelRequest：Lua取消请求，公开事件
    request_id: String--被取消请求的标识
    vm_id: Option<LuaVmId>--已创建VM时指定VM标识，尚未创建时为空
    impl Event for LuaRuntimeCancelRequest

LuaVmMessage：Lua VM消息，公开事件
    vm_id: LuaVmId--目标长期VM
    value: LuaValue--投递给VM消息邮箱的结构化值
    impl Event for LuaVmMessage

LuaVmMessageReceiveRequest：长期Lua VM邮箱读取请求，公开事件
    id: String--调用方生成的唯一请求ID
    vm_id: LuaVmId--目标长期VM
    impl Event + Clone for LuaVmMessageReceiveRequest

LuaVmMessageReceived：长期Lua VM邮箱读取响应，公开事件
    id: String--原读取请求ID
    vm_id: LuaVmId--原目标长期VM
    result: Result<LuaValue, LuaRuntimeError>--最早的邮箱值或稳定运行时错误
    impl Event + Clone for LuaVmMessageReceived

LuaVmStarted：Lua VM启动通知，公开事件--VM已经创建、环境已经完整安装且程序已经成功加载
    request_id: String--原执行请求标识
    vm_id: LuaVmId--新VM标识
    owner: LuaVmOwner--原请求归属
    impl Event for LuaVmStarted

LuaRuntimeTaskFinished：Lua任务完成事件，公开事件--通知运行时所属业务清理请求索引和VM归属
    request_id: String--完成请求的标识
    vm_id: Option<LuaVmId>--已经分配时为VM标识，请求在调度前失败时为空
    owner: LuaVmOwner--原请求归属
    state: LuaVmState--任务终态
    error: Option<LuaRuntimeError>--失败时的稳定错误，正常完成或取消时为空
    impl Event for LuaRuntimeTaskFinished
```

# types

types 放除事件和错误外的其余类型、环境注册表和运行时句柄。

## 类型

公开：
```text
LuaValue：Lua结果值，公开枚举--运行时边界允许传输的无环数据
    Nil
    Boolean(bool)
    Integer(i64)
    Number(f64)
    String(String)
    Array(Vec<LuaValue>)
    Object(BTreeMap<String, LuaValue>)

LuaVmOwner：VM归属，公开结构体
    owner_id: String--归属资源或任务的稳定标识

LuaProgram：Lua程序，公开结构体
    source: String--UTF-8 Lua源码
    origin: String--日志和错误定位用来源名
    entry: Option<String>--入口函数名；空表示直接执行源码Chunk
    libraries: LuaStandardLibraries--显式允许的Lua标准库集合

LuaStandardLibraries：Lua标准库集合，公开枚举
    Safe--只开放不直接访问文件、进程、动态库和调试器的基础库
    Full--开放完整Lua标准库

LuaEnvironmentContext：环境上下文，公开结构体
    request_id: String--本次请求标识
    owner: LuaVmOwner--请求归属
    values: BTreeMap<String, LuaValue>--调用方提供的结构化只读数据

LuaEnvironment：Lua环境，公开结构体
    globals: Vec<LuaGlobalBinding>--全局绑定
    modules: Vec<LuaModuleBinding>--模块绑定

LuaGlobalBinding：Lua全局绑定，公开结构体
    name: String--全局名称
    binding: LuaBindingValue--绑定内容

LuaBindingValue：Lua绑定内容，公开枚举
    Value(LuaValue)--可序列化的只读值
    Function(Arc<dyn LuaHostFunction>)--宿主异步函数

LuaModuleBinding：Lua模块绑定，公开结构体
    name: String--模块名称
    exports: BTreeMap<String, LuaBindingValue>--模块导出值和函数

LuaHostFunction：宿主函数，公开trait对象类型
    继承：Send + Sync + 'static
    call(&self, arguments: LuaValue, context: LuaEnvironmentContext, cancel: CancellationToken) -> HostFuture
        调用宿主能力：公开方法，Lua调用点必须等待Future产生唯一结果

HostFuture：宿主Future，公开类型别名
    Pin<Box<dyn Future<Output = Result<LuaValue, LuaRuntimeError>> + Send + 'static>>

CancellationToken：取消令牌，公开结构体
    is_cancelled(&self) -> bool
        查询取消：公开方法
    cancelled(&self) -> impl Future<Output = ()>
        等待取消：公开方法
    cancel(&self)
        请求取消：crate公开方法

LuaScheduler：调度模式，公开枚举
    LongRunning--长期服务或控制程序
    WorkerPool--普通短任务
    DedicatedThread--可能阻塞或需要线程隔离的任务

LuaVmState：VM状态，公开枚举
    Starting
    Running
    Waiting
    Completed
    Failed
    Cancelled

LuaRuntimeReply：Lua运行时回执，公开结构体
    new(sender: oneshot::Sender<LuaRuntimeResult>) -> Self
        构造回执：公开关联函数
    take(&self) -> Option<oneshot::Sender<LuaRuntimeResult>>
        取得发送器：crate公开方法
    fail(&self, error: LuaRuntimeError)
        发送失败：crate公开方法

LuaRuntimeResult：Lua执行结果，公开枚举
    Completed { value: LuaValue }
    Failed { error: LuaRuntimeError }
    Cancelled

LuaEnvironmentProvider：环境提供器，公开trait
    继承：Send + Sync + 'static
    name(&self) -> &str
        获取名称：公开方法
    provide(&self, context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>
        生成环境：公开方法，不访问World

LuaEnvironmentRegistry：环境提供器注册表，公开结构体
    providers: BTreeMap<String, Box<dyn LuaEnvironmentProvider>>--提供器，私有
    register(&mut self, provider: Box<dyn LuaEnvironmentProvider>) -> Result<(), LuaRuntimeError>
        注册提供器：公开方法
    collect(&self, names: &[String], context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>
        收集环境：公开方法，严格按names顺序合并，冲突立即失败

LuaVmSession：VM会话，公开结构体
    vm_id: LuaVmId--会话标识
    owner: LuaVmOwner--会话归属
    state: LuaVmState--当前状态
    created_at: Instant--创建时间
    last_activity: Instant--最近执行或宿主调用时间

LuaRuntimeConfig：运行时配置，公开Resource
    max_source_bytes: usize--单个程序源码上限
    max_result_bytes: usize--序列化结果上限
    default_timeout: Duration--未指定截止时间时使用的执行时长
    queue_capacity: usize--调度队列容量
    worker_count: usize--WorkerPool默认并发数
    impl Default for LuaRuntimeConfig
    impl Resource for LuaRuntimeConfig

LuaRuntimeHandle：运行时句柄，公开Resource
    events: RuntimeEventSender--向Runtime发送执行和取消事件，crate公开
    environments: Arc<RwLock<LuaEnvironmentRegistry>>--环境提供器注册表，crate公开
    submit(&self, request: LuaRuntimeRequest) -> Result<(), LuaRuntimeError>
        提交执行：公开方法
    register_long_running(&self, request: LuaRuntimeRequest) -> Result<(), LuaRuntimeError>
        注册长期VM：公开方法
    stop_long_running(&self, owner_id: &str) -> Result<(), LuaRuntimeError>
        停止长期VM：公开方法
    send_message(&self, vm_id: LuaVmId, value: LuaValue) -> Result<(), LuaRuntimeError>
        投递长期VM消息：公开方法
    receive_message(&self, id: String, vm_id: LuaVmId) -> Result<(), LuaRuntimeError>
        请求邮箱消息：公开方法
    cancel(&self, request_id: &str) -> Result<(), LuaRuntimeError>
        取消执行：公开方法
    register_provider(&self, provider: Box<dyn LuaEnvironmentProvider>) -> Result<(), LuaRuntimeError>
        注册环境提供器：公开方法
    impl Resource for LuaRuntimeHandle
```

crate公开：
```text
LuaRuntimeState：运行时状态，crate公开Resource
    next_vm: AtomicU64--VM ID递增来源
    sessions: HashMap<LuaVmId, LuaVmSession>--活跃会话
    requests: HashMap<String, CancellationToken>--请求取消令牌
    owners: HashMap<String, String>--请求ID到owner_id
    mailboxes: HashMap<LuaVmId, VecDeque<LuaValue>>--长期VM FIFO邮箱
    receives: HashMap<LuaVmId, VecDeque<LuaVmMessageReceiveRequest>>--pending receive
    impl Resource for LuaRuntimeState
```

# error

error 放 Error 类型和公开错误分类。

## 类型

公开：
```text
LuaRuntimeError：运行时错误，公开枚举
    RuntimeClosed
    InvalidRequest(String)
    SourceTooLarge
    ResultTooLarge
    ProviderAlreadyRegistered(String)
    EnvironmentProviderNotFound(String)
    EnvironmentConflict(String)
    EnvironmentFailed(String)
    SchedulerUnavailable
    Timeout
    Cancelled
    VmCreationFailed(String)
    VmExecutionFailed(String)
    impl fmt::Display for LuaRuntimeError
    impl std::error::Error for LuaRuntimeError
    impl Clone for LuaRuntimeError
```

## 逻辑

```text
请求执行：
    业务插件显式构造LuaRuntimeRequest并提交
    lua_runtime_request_system克隆事件并逐条交给handle_lua_runtime_request
    校验失败立即reply.fail并发送LuaRuntimeTaskFinished
    成功分配LuaVmId与CancellationToken，LongRunning先发LuaVmStarted
    独立线程创建Lua VM、安装环境、执行脚本，结束后销毁VM
    每个LuaRuntimeRequest最终恰好发送一个LuaRuntimeTaskFinished

环境组合：
    Provider只生成环境描述，不直接修改Lua VM
    Registry严格按请求声明顺序合并Provider结果
    全局或模块名称冲突是配置错误，整次请求失败

长期VM邮箱：
    LuaVmMessage投递到FIFO邮箱，不等待Lua处理
    LuaVmMessageReceiveRequest在邮箱为空时挂起，至多一个pending receive
    VM停止时关闭邮箱并完成pending receive为错误

生命周期与取消：
    VM在Completed、Failed或Cancelled后立即销毁
    LongRunning只表示单次程序可以长期运行，不表示跨请求复用VM
    取消不强行中断宿主Future，先发取消令牌
    LuaRuntimeReply在System移交任务前保证失败可回执
```

## 边界

```text
LuaRuntimePlugin不解析MCL，不创建Agent，不查找ResourceMap，不注册工具，不发送WebSocket消息。
LuaRuntimePlugin不实现文件、HTTP、Shell或MCL操作；这些能力必须由LuaEnvironmentProvider注入。
业务插件通过Provider和LuaRuntimeHandle接入运行时，结果转换由业务插件自身完成。
```
