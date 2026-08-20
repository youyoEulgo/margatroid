# LuaRuntimePlugin

## 类型

公开：
```text
LuaRuntimePlugin：Lua运行时插件，公开单元结构体--统一创建、调度、执行和销毁Lua虚拟机，不包含任何业务领域语义
    impl Plugin for LuaRuntimePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装运行时：公开插件入口
            行为：
                要求AsyncRuntimePlugin已经安装
                插入LuaRuntimeConfig、LuaEnvironmentRegistry和LuaRuntimeHandle
                注册LuaRuntimeRequest、LuaRuntimeCancelRequest、LuaVmMessage、LuaVmMessageReceiveRequest、LuaVmMessageReceived、LuaVmStarted和LuaRuntimeTaskFinished事件
                在RuntimePlugin::UPDATE挂载lua_runtime_request_system、lua_runtime_cancel_system、lua_vm_message_system和lua_runtime_result_system
                启动长期调度器服务；服务只负责从调度队列取请求，不直接访问World
```

```text
LuaVmId：虚拟机标识，公开元组结构体--一次Lua执行会话的稳定标识
    0: u64--运行时内递增标识
    类型归属：实际定义在共享types crate并由LuaRuntimePlugin重新导出，使Agent能够保存VM标识且LuaRuntimePlugin不反向依赖AgentPlugin

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

LuaRuntimeCancelRequest：Lua取消请求，公开事件--请求取消一个尚未完成的执行
    request_id: String--被取消请求的标识
    vm_id: Option<LuaVmId>--已创建VM时指定VM标识，尚未创建时为空
    impl Event for LuaRuntimeCancelRequest

LuaVmMessage：Lua VM消息，公开事件--向已经启动的长期Lua VM邮箱投递一个宿主值
    vm_id: LuaVmId--目标长期VM
    value: LuaValue--投递给VM消息邮箱的结构化值
    impl Event for LuaVmMessage

LuaVmMessageReceiveRequest：长期Lua VM邮箱读取请求，公开事件--业务Effect按FIFO取得下一项
    id: String--调用方生成的唯一请求ID
    vm_id: LuaVmId--目标长期VM
    impl Event for LuaVmMessageReceiveRequest
    impl Clone for LuaVmMessageReceiveRequest

LuaVmMessageReceived：长期Lua VM邮箱读取响应，公开事件--无论成功失败都恰好响应一次
    id: String--原读取请求ID
    vm_id: LuaVmId--原目标长期VM
    result: Result<LuaValue, LuaRuntimeError>--最早的邮箱值或稳定运行时错误
    impl Event for LuaVmMessageReceived
    impl Clone for LuaVmMessageReceived

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

LuaRuntimeResult：Lua执行结果，公开枚举--只包含运行时结果，不包含业务工具或MCL类型
    Completed { value: LuaValue }--脚本正常返回的可序列化值
    Failed { error: LuaRuntimeError }--脚本、环境、超时或调度失败
    Cancelled--调用方或运行时关闭导致的取消

LuaRuntimeReply：Lua运行时回执，公开结构体--允许事件System取得一次性发送器并保证最多发送一次结果
    sender: Arc<Mutex<Option<oneshot::Sender<LuaRuntimeResult>>>>--私有回执槽位
    new(sender: oneshot::Sender<LuaRuntimeResult>) -> Self
        构造回执：公开关联函数，将sender写入共享槽位
    take(&self) -> Option<oneshot::Sender<LuaRuntimeResult>>
        取得发送器：crate公开方法，取出后槽位为空
    fail(&self, error: LuaRuntimeError)
        发送失败：crate公开方法，取出发送器并发送Failed；已取出时不执行操作

LuaValue：Lua结果值，公开枚举--运行时边界允许传输的无环数据
    Nil
    Boolean(bool)
    Integer(i64)
    Number(f64)
    String(String)
    Array(Vec<LuaValue>)
    Object(BTreeMap<String, LuaValue>)

LuaRuntimeHandle：运行时句柄，公开结构体--向业务插件提交执行、取消和注册环境提供器
    events: RuntimeEventSender--向Runtime发送执行和取消事件
    environments: Arc<RwLock<LuaEnvironmentRegistry>>--环境提供器注册表
    submit(&self, request: LuaRuntimeRequest) -> Result<(), LuaRuntimeError>
        提交执行：公开方法
        行为：调用events发送LuaRuntimeRequest并唤醒Runtime；Runtime关闭时返回RuntimeClosed
    register_long_running(&self, request: LuaRuntimeRequest) -> Result<(), LuaRuntimeError>
        注册长期VM：公开方法
        行为：要求request.scheduler为LongRunning且request.owner.owner_id非空；拒绝同一owner_id已有未结束长期VM；将请求提交到LongRunning调度器
    stop_long_running(&self, owner_id: &str) -> Result<(), LuaRuntimeError>
        停止长期VM：公开方法
        行为：按owner_id找到长期VM并提交取消；不存在时按幂等成功处理
    send_message(&self, vm_id: LuaVmId, value: LuaValue) -> Result<(), LuaRuntimeError>
        投递长期VM消息：公开方法
        行为：发送LuaVmMessage并唤醒目标VM；VM不存在、已停止或邮箱已关闭时返回错误
    receive_message(&self, id: String, vm_id: LuaVmId) -> Result<(), LuaRuntimeError>
        请求邮箱消息：公开方法
        行为：发送LuaVmMessageReceiveRequest并唤醒目标长期VM；成功只表示读取请求已登记，结果通过LuaVmMessageReceived返回
    cancel(&self, request_id: &str) -> Result<(), LuaRuntimeError>
        取消执行：公开方法
        行为：调用events发送LuaRuntimeCancelRequest并唤醒Runtime；成功只表示请求已提交，不保证目标仍在运行
    register_provider(&self, provider: Box<dyn LuaEnvironmentProvider>) -> Result<(), LuaRuntimeError>
        注册环境提供器：公开方法
        行为：按provider.name注册；同名提供器返回ProviderAlreadyRegistered
```

```text
LuaProgram：Lua程序，公开结构体--一次执行所需的源码和来源信息
    source: String--UTF-8 Lua源码
    origin: String--日志和错误定位用来源名，不要求是文件路径
    entry: Option<String>--入口函数名；空表示直接执行源码Chunk，非空表示执行Chunk后调用该全局函数
    libraries: LuaStandardLibraries--显式允许的Lua标准库集合

LuaStandardLibraries：Lua标准库集合，公开枚举--程序创建VM时开放的原生Lua能力
    Safe--只开放不直接访问文件、进程、动态库和调试器的基础库
    Full--开放完整Lua标准库，适合用户明确信任的工具程序

LuaVmOwner：VM归属，公开结构体--用于生命周期隔离和日志关联，不携带业务对象引用
    owner_id: String--归属资源或任务的稳定标识

LuaEnvironmentContext：环境上下文，公开结构体--提供器读取的只读调用数据
    request_id: String--本次请求标识
    owner: LuaVmOwner--请求归属
    values: BTreeMap<String, LuaValue>--调用方提供的结构化只读数据

LuaEnvironmentProvider：环境提供器，公开trait--把一个独立能力安装到Lua VM
    继承：Send + Sync + 'static
    name(&self) -> &str
        获取名称：公开方法，名称用于注册、排序和冲突诊断
    provide(&self, context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>
        生成环境：公开方法，根据上下文创建本次VM的环境描述，不访问World

LuaEnvironment：Lua环境，公开结构体--待安装的全局值和模块
    globals: Vec<LuaGlobalBinding>--全局绑定
    modules: Vec<LuaModuleBinding>--模块绑定

LuaGlobalBinding：Lua全局绑定，公开结构体--向全局表写入一个值或函数
    name: String--全局名称，使用点号路径时按路径创建表
    binding: LuaBindingValue--绑定内容

LuaBindingValue：Lua绑定内容，公开枚举--全局或模块可以安装的值
    Value(LuaValue)--可序列化的只读值
    Function(Arc<dyn LuaHostFunction>)--宿主异步函数

LuaModuleBinding：Lua模块绑定，公开结构体--向require可见的模块表注册能力
    name: String--模块名称
    exports: BTreeMap<String, LuaBindingValue>--模块导出值和函数

LuaHostFunction：宿主函数，公开trait对象类型--Lua侧以普通阻塞函数调用、宿主侧异步执行的受限操作
    call(&self, arguments: LuaValue, context: LuaEnvironmentContext, cancel: CancellationToken) -> HostFuture
        调用宿主能力：公开方法
        行为：返回Future；Lua调用点必须暂停当前VM并等待该Future产生唯一结果，不向Lua暴露异步与同步两套调用方式；等待期间把控制权交还异步运行时，不阻塞调度线程；结果到达后恢复同一VM并将LuaValue作为函数返回值，错误作为Lua错误抛出；实现必须响应cancel

HostFuture：宿主Future，公开类型别名--宿主函数返回的、可取消的异步结果
    等于 Pin<Box<dyn Future<Output = Result<LuaValue, LuaRuntimeError>> + Send + 'static>>

CancellationToken：取消令牌，公开结构体--执行任务和宿主函数共享的协作式取消标记
    is_cancelled(&self) -> bool
        查询取消：公开方法，已请求取消时返回真
    cancelled(&self) -> impl Future<Output = ()>
        等待取消：公开方法，在取消发生前挂起调用方

LuaScheduler：调度模式，公开枚举--决定VM由哪个执行隔离承载
    LongRunning--长期服务或控制程序，使用独立线程且无显式deadline时不设置默认时限
    WorkerPool--普通短任务，使用配置指定容量的有界VM Worker池
    DedicatedThread--可能阻塞或需要线程隔离的任务，使用独立OS线程

LuaVmState：VM状态，公开枚举--运行时可观察状态
    Starting
    Running
    Waiting
    Completed
    Failed
    Cancelled

LuaRuntimeError：运行时错误，公开枚举--稳定描述运行时边界错误
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
    impl Clone for LuaRuntimeError
```

crate公开：
```text
LuaRuntimeConfig：运行时配置，crate公开结构体--限制资源、超时和队列容量
    max_source_bytes: usize--单个程序源码上限
    max_result_bytes: usize--序列化结果上限
    default_timeout: Duration--未指定截止时间时使用的执行时长
    queue_capacity: usize--调度队列容量
    worker_count: usize--WorkerPool默认并发数

LuaEnvironmentRegistry：环境提供器注册表，crate公开结构体--按名称保存提供器并生成确定顺序的环境
    providers: BTreeMap<String, Box<dyn LuaEnvironmentProvider>>--提供器
    register(&mut self, provider: Box<dyn LuaEnvironmentProvider>) -> Result<(), LuaRuntimeError>
        注册提供器：crate公开方法
        行为：拒绝空名称和重复名称，成功后按名称排序
    collect(&self, providers: &[String], context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>
        收集环境：crate公开方法
        行为：
            拒绝重复提供器名称
            严格按providers顺序查找并调用提供器；名称不存在时返回EnvironmentProviderNotFound
            合并绑定；全局名或模块名冲突立即失败

LuaVmSession：VM会话，crate公开结构体--单一调度任务独占的Lua VM及其状态
    vm_id: LuaVmId--会话标识
    owner: LuaVmOwner--会话归属
    state: LuaVmState--当前状态
    created_at: Instant--创建时间
    last_activity: Instant--最近执行或宿主调用时间

```

## 函数

私有：
```text
lua_runtime_request_system(world: &mut World)
    接收执行请求：私有System
    行为：
        逐个读取LuaRuntimeRequest
        校验request_id、可选程序入口、源码大小和截止时间
        使用请求providers调用LuaEnvironmentRegistry::collect生成本次环境
        按LuaScheduler将请求放入LongRunning、WorkerPool或DedicatedThread队列
        将reply中的oneshot发送器取出并随调度任务移动
        任一步失败或队列拒绝时调用reply.fail发送LuaRuntimeResult::Failed，并发送vm_id为空的LuaRuntimeTaskFinished
        不在System内创建或执行Lua VM

lua_runtime_cancel_system(world: &mut World)
    接收取消请求：私有System
    行为：
        在调度注册表中按request_id查找任务
        标记取消令牌并唤醒对应调度器
        VM在下一个指令检查点和每次宿主调用前检查令牌
        已完成任务不重复发送结果

lua_vm_message_system(world: &mut World)
    投递长期VM消息：私有System
    行为：
        按事件顺序读取LuaVmMessage和LuaVmMessageReceiveRequest
        每个长期VM保存一个FIFO值队列和至多一个pending receive；receive重复时立即发送InvalidRequest响应
        收到receive且队列非空时取出最早值并发送LuaVmMessageReceived；队列为空时保存请求
        收到message且存在pending receive时直接配对并发送LuaVmMessageReceived；否则把value追加到FIFO队列
        VM停止、失败或邮箱关闭时，以对应LuaRuntimeError完成pending receive并清空未消费队列
        VM不存在时message记录稳定错误，receive发送Err响应；不执行Lua代码

lua_runtime_result_system(world: &mut World)
    收集运行时结果：私有System
    行为：
        读取LuaRuntimeTaskFinished
        清理请求索引和临时VM
        记录稳定错误信息
        不将结果转换为ToolCallResponse、AgentMessage或MCL领域事件

create_vm(program: &LuaProgram, limits: &LuaRuntimeConfig) -> Result<Lua, LuaRuntimeError>
    创建VM：私有函数
    行为：创建独立Lua 5.4 VM，严格按program.libraries安装标准库并设置指令和内存限制；失败返回VmCreationFailed

install_environment(lua: &Lua, environment: LuaEnvironment, context: LuaEnvironmentContext) -> Result<(), LuaRuntimeError>
    安装环境：私有函数
    行为：按确定顺序安装全局值、异步宿主函数和模块；禁止覆盖已存在的绑定；失败时销毁VM并返回错误

execute_vm(session: LuaVmSession, program: LuaProgram, environment: LuaEnvironment, cancel: CancellationToken) -> LuaRuntimeResult
    执行VM：私有异步函数
    行为：
        创建VM并安装环境
        编译source，成功后发送LuaVmStarted
        entry为空时异步执行源码Chunk；entry非空时先执行Chunk，再查找并异步调用对应全局函数
        Lua调用任意宿主函数时都按阻塞函数语义停在调用点；运行时挂起当前VM并等待宿主Future，把控制权交还异步运行时；Future完成后恢复同一VM并从调用点返回结果
        在指令Hook、宿主调用前和截止时间到达时检查cancel
        将返回值转换为LuaValue并检查结果大小
        正常返回Completed；脚本异常、超时或取消分别返回Failed或Cancelled
        函数结束后立即销毁VM
```

公开：
```text
new_lua_runtime_request(request_id: String, owner: LuaVmOwner, program: LuaProgram, context: LuaEnvironmentContext, providers: Vec<String>, scheduler: LuaScheduler, reply: oneshot::Sender<LuaRuntimeResult>) -> LuaRuntimeRequest
    构造请求：公开函数
    行为：使用sender构造LuaRuntimeReply，再构造不含隐式业务字段的LuaRuntimeRequest；调用方必须显式提供所有上下文
```

## 逻辑

```text
插件安装：
    LuaRuntimePlugin要求AsyncRuntimePlugin已经存在
    插入配置、环境注册表和运行时句柄
    使用MECS异步服务启动调度协调器
    启动三个调度隔离：长期任务、有限Worker池、独立线程
    协调器可以运行在MECS异步执行器，但Lua字节码只在LuaRuntimePlugin自己的工作线程执行
    WorkerPool由固定数量OS线程组成，每个线程持有自己的Tokio current_thread runtime并逐个执行临时VM
    LongRunning和DedicatedThread分别为请求创建OS线程和Tokio current_thread runtime；前者允许无默认时限，后者仍使用默认时限
    Lua纯计算、死循环和同步宿主调用不得阻塞Axum或MECS异步执行器

请求执行：
    业务插件显式构造LuaRuntimeRequest并提交
    request_system校验请求并收集全部EnvironmentProvider
    调度器为请求分配唯一LuaVmId和取消令牌
    每个请求创建一个独立Lua VM，任务结束后销毁
    单一任务独占一个Lua VM；同一VM不接受并发调用
    VM创建、环境安装或源码编译失败时不发送LuaVmStarted
    每个LuaRuntimeRequest最终都恰好发送一个LuaRuntimeTaskFinished，包括VM分配前失败的请求
    LuaVmStarted只表示运行时承载已经建立，不表示业务脚本完成了自己的初始化
    VM执行结果通过请求reply返回，并发送LuaRuntimeTaskFinished供运行时清理

环境组合：
    Provider只生成环境描述，不直接修改Lua VM
    请求显式列出所需Provider；未列出的能力不进入VM
    Registry严格按请求声明顺序合并Provider结果
    全局或模块名称冲突是配置错误，整次请求失败，不部分执行
    业务插件可以注入MCL句柄、文件句柄或工具句柄，但LuaRuntimePlugin不识别这些语义

宿主函数调用语义：
    Lua代码中所有注入的宿主函数都是普通函数调用，不存在await、异步变体或fire-and-forget变体
    每次调用都必须产生一个返回结果；Lua可以把结果赋给变量，也可以忽略返回值，但忽略返回值不会改变等待和执行语义
    调用期间只暂停当前Lua VM，不阻塞承载调度器的异步线程；运行时在宿主Future完成后恢复同一VM
    同一VM一次只推进一个Lua调用栈，因此宿主函数响应返回前不会继续执行该调用点之后的Lua语句

生命周期与取消：
    VM在Completed、Failed或Cancelled后立即销毁
    LongRunning只表示单次程序可以长期运行，不表示跨请求复用VM
    跨请求持久VM不属于首版设计，未来必须以新的显式调度模式增加
    取消不强行中断宿主Future，先发取消令牌；超时后调度器终止任务并回收VM
    任何异常路径最多向reply发送一次结果，运行时关闭时未完成请求统一返回Cancelled
    LuaRuntimeReply在System移交任务前保证失败可回执，调度任务取得发送器后负责所有后续回执

长期VM邮箱：
    LongRunning请求创建VM后，同时创建有界LuaVmMessage邮箱
    Base Lua通过宿主提供的start或receive函数异步读取邮箱值
    LuaVmMessage只负责把值投递到邮箱，不等待Lua处理完成
    VM停止时关闭邮箱；尚未投递的消息返回RuntimeClosed或MessageQueueClosed

边界：
    LuaRuntimePlugin不解析MCL，不创建Agent，不查找ResourceMap，不注册工具，不发送WebSocket消息
    LuaRuntimePlugin不实现文件、HTTP、Shell或MCL操作；这些能力必须由LuaEnvironmentProvider注入
    业务插件通过Provider和LuaRuntimeHandle接入运行时，结果转换由业务插件自身完成
```
