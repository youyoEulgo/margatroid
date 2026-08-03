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

函数：使用二级标题，按私有、crate公开和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# AsyncRuntimePlugin

异步处理分为两种模式：

```text
事件模式：事件传递数据，预先注册的异步System持有固定处理闭包
闭包模式：一次性异步闭包被包装成同步分发闭包，复用ClosurePlugin的ClosureSystem
```

## 类型

公开：
```text
AsyncTaskError：异步任务错误，公开枚举--可通过From转换进入开发者选择的响应错误类型E
    Panicked {
        message: String--panic信息
    }
    Cancelled
    from_join_error(error: JoinError) -> Self
        从JoinError构造：crate公开关联函数，error是受监督任务的结束状态
        行为：panic时提取信息构造Panicked，否则构造Cancelled
    impl fmt::Display for AsyncTaskError
        Display：公开trait实现
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化任务错误：formatter接收稳定错误描述
            行为：输出任务panic信息或任务已取消
    impl std::error::Error for AsyncTaskError
        Error：公开trait实现

AsyncRuntimeError：异步Runtime错误，公开枚举--描述异步基础设施的配置错误与运行错误
    RuntimePluginMissing
    AsyncRuntimePluginMissing
    AsyncRuntimePluginAlreadyInstalled
    AsyncSystemAlreadyRegistered {
        event_type: &'static str--事件类型名
    }
    AsyncSystemNotRegistered {
        event_type: &'static str--事件类型名
    }
    ExecutorThreadStartFailed {
        source: io::Error--底层IO错误
    }
    ExecutorRuntimeBuildFailed {
        source: io::Error--底层IO错误
    }
    ExecutorDisconnected
    panic(self) -> !
        终止：crate公开方法，消费当前AsyncRuntimeError并使用Display文本终止执行
        行为：使用自身Display描述触发panic
    impl fmt::Display for AsyncRuntimeError
        Display：公开trait实现
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化Runtime错误：formatter接收稳定错误描述
            行为：输出包含错误类型与上下文的描述
    impl std::error::Error for AsyncRuntimeError
        Error：公开trait实现
        source(&self) -> Option<&(dyn std::error::Error + 'static)>
            获取错误来源：返回执行线程启动或Runtime构建的底层IO错误
            行为：其他变体返回None

AsyncRuntimeHandle：异步Runtime句柄，公开结构体--由World持有，只通过受控扩展接口提交异步任务
    sender: Option<UnboundedSender<ErasedExecutionTask>>--任务发送端，私有
    thread: Option<JoinHandle<()>>--异步线程，私有
    new(sender: UnboundedSender<ErasedExecutionTask>, thread: JoinHandle<()>) -> Self
        构造句柄：crate公开关联函数，sender和thread组成执行器所有权
        行为：将两个参数包装为Some
    spawn(&self, task: ErasedExecutionTask)
        提交任务：crate公开方法，task是类型擦除后的执行任务
        行为：发送到异步线程；通道断开时报告AsyncRuntimeError::ExecutorDisconnected
    impl Drop for AsyncRuntimeHandle
        Drop：公开trait实现
        drop(&mut self)
            回收执行器：关闭任务通道并等待专用线程退出
            行为：先take sender，再take并join thread
    impl Resource for AsyncRuntimeHandle
        Resource：公开trait实现

AsyncContext：异步上下文，公开结构体--由AsyncRuntime自动注入需要上下文的任务
    events: RuntimeEventSender--Runtime事件发送器，私有
    new(events: RuntimeEventSender) -> Self
        构造上下文：crate公开关联函数，events提供跨线程事件发送能力
        行为：持有events
    send_event<E>(&self, event: E)
        发送事件：公开泛型方法，event是立即发送的中间事件
        约束：E: Event
        行为：调用RuntimeEventSender::send_event并唤醒Runtime
    send_event_after<E>(&self, event: E, delay: u64)
        延迟发送事件：公开泛型方法，delay指定额外延迟帧数
        约束：E: Event
        行为：调用RuntimeEventSender::send_event_after并唤醒Runtime

AsyncTask<T, E, Args>：闭包模式任务适配层，公开trait--接受有上下文或无上下文的一次性异步闭包
    继承：Send + 'static
    Future: Future<Output = Result<T, E>> + Send + 'static--关联类型
    run(self, context: AsyncContext) -> Self::Future
        运行任务：公开方法，context由AsyncRuntime注入
    impl<T, E, Task, TaskFuture> AsyncTask<T, E, ()> for Task
        无上下文闭包：公开泛型trait实现
        约束：
            Task: FnOnce() -> TaskFuture + Send + 'static
            TaskFuture: Future<Output = Result<T, E>> + Send + 'static
        run(self, _context: AsyncContext) -> Self::Future
            运行任务：忽略注入的上下文并调用self()
    impl<T, E, Task, TaskFuture> AsyncTask<T, E, (AsyncContext,)> for Task
        有上下文闭包：公开泛型trait实现
        约束：
            Task: FnOnce(AsyncContext) -> TaskFuture + Send + 'static
            TaskFuture: Future<Output = Result<T, E>> + Send + 'static
        run(self, context: AsyncContext) -> Self::Future
            运行任务：将context传入self

AsyncEventHandler<Request, T, E, Args>：事件模式处理器适配层，公开trait--将固定闭包与Request事件类型绑定
    继承：Send + Sync + 'static
    Future: Future<Output = Result<T, E>> + Send + 'static--关联类型
    run(&self, request: Request, context: AsyncContext) -> Self::Future
        运行处理器：公开方法，request是已取得所有权的请求事件，context由AsyncRuntime注入
    impl<Request, T, E, Handler, HandlerFuture> AsyncEventHandler<Request, T, E, ()> for Handler
        无上下文处理器：公开泛型trait实现
        约束：
            Handler: Fn(Request) -> HandlerFuture + Send + Sync + 'static
            HandlerFuture: Future<Output = Result<T, E>> + Send + 'static
        run(&self, request: Request, _context: AsyncContext) -> Self::Future
            运行处理器：忽略上下文并调用self(request)
    impl<Request, T, E, Handler, HandlerFuture> AsyncEventHandler<Request, T, E, (AsyncContext,)> for Handler
        有上下文处理器：公开泛型trait实现
        约束：
            Handler: Fn(Request, AsyncContext) -> HandlerFuture + Send + Sync + 'static
            HandlerFuture: Future<Output = Result<T, E>> + Send + 'static
        run(&self, request: Request, context: AsyncContext) -> Self::Future
            运行处理器：将request和context传入self

AsyncRuntimePlugin：异步Runtime插件，公开单元结构体
    impl Plugin for AsyncRuntimePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装专用异步执行器与事件模式注册表
            行为：
                RuntimeHandle不存在时报告RuntimePluginMissing
                AsyncRuntimeHandle或AsyncRegistry已存在时报告AsyncRuntimePluginAlreadyInstalled
                调用start_executor并插入AsyncRuntimeHandle
                创建并插入AsyncRegistry

AppAsyncExt：App异步扩展，公开trait--挂载持有固定异步处理闭包的事件模式System
    add_async_system<Request, T, E, Handler, Args>(&mut self, schedule: &str, handler: Handler) -> &mut Self
        添加事件模式System：公开泛型方法，schedule指定处理Request的阶段，handler是可重复调用的固定异步闭包
        约束：
            Request: Event
            T: Send + Sync + 'static
            E: From<AsyncTaskError> + Send + Sync + 'static
            Handler: AsyncEventHandler<Request, T, E, Args>
            Args: 'static
        行为：
            AsyncRuntimePlugin未安装时报告AsyncRuntimePluginMissing
            Request已注册事件模式System时报告AsyncSystemAlreadyRegistered
            在AsyncRegistry中注册Request的TypeId和类型名
            向schedule添加AsyncEventSystem<Request, T, E, Handler, Args>
            返回App可变引用
    impl AppAsyncExt for App
        AppAsyncExt for App：公开trait实现
        add_async_system<Request, T, E, Handler, Args>(&mut self, schedule: &str, handler: Handler) -> &mut Self
            添加事件模式System：按trait定义注册类型并挂载System

WorldAsyncExt：World异步扩展，公开trait--发送事件模式请求、提交闭包模式请求或启动长期服务
    send_async_event<Request>(&self, event: Request)
        发送Async事件模式请求：公开泛型方法，event是交给固定异步System的数据
        约束：Request: Event
        行为：确认Request已注册，构造mode为Async的AsyncEventRequest<Request>，调用WorldEventExt::send_event发送请求并由Runtime统一唤醒，不操作阀
    send_await_event<Request>(&self, event: Request)
        发送Await事件模式请求：公开泛型方法，event是交给固定异步System的数据
        约束：Request: Event
        行为：确认Request已注册，构造mode为Await的AsyncEventRequest<Request>，调用WorldEventExt::send_event发送请求并由Runtime统一唤醒，不操作阀
    send_async_closure<T, E, Task, Args>(&self, schedule: &str, task: Task)
        提交Async闭包模式请求：公开泛型方法，schedule指定转发阶段，task是一次性异步闭包
        约束：
            T: Send + Sync + 'static
            E: From<AsyncTaskError> + Send + Sync + 'static
            Task: AsyncTask<T, E, Args>
        行为：构造捕获task的同步分发闭包，闭包调用dispatch_closure_task并使用Async模式；调用WorldClosureExt::send_closure发送到schedule；发送时不创建pending且不操作阀
    send_await_closure<T, E, Task, Args>(&self, schedule: &str, task: Task)
        提交Await闭包模式请求：公开泛型方法，schedule指定转发阶段，task是一次性异步闭包
        约束：
            T: Send + Sync + 'static
            E: From<AsyncTaskError> + Send + Sync + 'static
            Task: AsyncTask<T, E, Args>
        行为：构造捕获task的同步分发闭包，闭包调用dispatch_closure_task并使用Await模式；调用WorldClosureExt::send_closure发送到schedule；发送时不创建pending且不操作阀
    spawn_async_service<Service>(&self, service: Service)
        启动长期异步服务：公开泛型方法，service由所属Plugin自行管理生命周期
        约束：Service: Future<Output = ()> + Send + 'static
        行为：从World取得AsyncRuntimeHandle并直接提交擦除后的Future；不创建请求、pending事件或响应
    impl WorldAsyncExt for World
        WorldAsyncExt for World：公开trait实现
        send_async_event<Request>(&self, event: Request)
            发送Async事件模式请求：按trait定义执行
        send_await_event<Request>(&self, event: Request)
            发送Await事件模式请求：按trait定义执行
        send_async_closure<T, E, Task, Args>(&self, schedule: &str, task: Task)
            提交Async闭包模式请求：按trait定义执行
        send_await_closure<T, E, Task, Args>(&self, schedule: &str, task: Task)
            提交Await闭包模式请求：按trait定义执行
        spawn_async_service<Service>(&self, service: Service)
            启动长期异步服务：按trait定义直接提交service
```

crate公开：
```text
ErasedExecutionTask：类型擦除执行任务，crate公开类型别名--等于Pin<Box<dyn Future<Output = ()> + Send + 'static>>

AsyncRegistry：异步注册表，crate公开结构体--记录已经挂载事件模式System的Request类型
    event_systems: HashMap<TypeId, &'static str>--事件模式System，Request TypeId到类型名的映射
    new() -> Self
        构造注册表：crate公开关联函数，构造event_systems为空的AsyncRegistry
    register_event_system<Request: Event>(&mut self) -> bool
        注册事件模式System：crate公开泛型方法，插入Request TypeId和类型名，首次返回true
    contains_event_system<Request: Event>(&self) -> bool
        查询事件模式System：crate公开泛型方法，Request TypeId存在时返回true
    impl Resource for AsyncRegistry
        Resource：crate公开trait实现
```

私有：
```text
AsyncMode：异步执行模式，私有枚举--由公开发送方法确定，不直接暴露给开发者
    Async--不阻塞下一帧，任务完成后唤醒Runtime
    Await--System开始任务时关阀，任务完成后开阀

AsyncEventRequest<Request: Event>：事件模式请求，私有结构体--携带Request所有权和执行模式
    event: Mutex<Option<Request>>--只能取出一次的请求事件
    mode: AsyncMode--异步执行模式
    new(event: Request, mode: AsyncMode) -> Self
        构造事件模式请求：私有关联函数，保存event和mode
    take_event(&self) -> Option<Request>
        取出请求事件：私有方法，首次调用转移所有权，后续返回None
    mode(&self) -> AsyncMode
        获取执行模式：私有方法，返回mode
    impl<Request: Event> Event for AsyncEventRequest<Request>
        Event：私有泛型trait实现

AsyncEventSystem<Request, T, E, Handler, Args>：事件模式System，私有结构体--读取固定Request类型并使用固定handler启动异步任务
    handler: Arc<Handler>--可重复调用的异步处理器
    marker: PhantomData<fn(Request, T, E, Args)>--泛型类型标记
    new(handler: Handler) -> Self
        构造事件模式System：私有泛型关联函数，将handler包装为Arc
    impl<Request, T, E, Handler, Args> System for AsyncEventSystem<Request, T, E, Handler, Args>
        System：私有泛型trait实现
        约束：与AppAsyncExt::add_async_system一致
        run(&mut self, world: &mut World)
            执行事件模式System：读取AsyncEventRequest<Request>，逐个取出Request并调用submit_event_task

```

## 函数

crate公开：
```text
start_executor() -> AsyncRuntimeHandle
    启动异步执行器：crate公开函数
    行为：创建任务通道和启动确认通道，启动专用线程，确认Tokio Runtime创建成功后返回AsyncRuntimeHandle
```

私有：
```text
submit_event_task<Request, T, E, Handler, Args>(world: &mut World, request: Request, mode: AsyncMode, handler: Arc<Handler>)
    提交事件模式任务：私有泛型函数，使用固定handler处理request
    约束：与AppAsyncExt::add_async_system一致
    行为：
        从world取得RuntimeHandle、RuntimeEventSender和AsyncRuntimeHandle
        调用World::emit_pending创建EventHandle<Result<T, E>>
        mode为Await时调用RuntimeHandle::close_gate
        使用RuntimeEventSender构造AsyncContext
        构造在异步线程中调用handler.run(request, context)的Future
        调用submit_supervised提交Future、pending句柄、mode和RuntimeHandle

dispatch_closure_task<T, E, Task, Args>(world: &mut World, task: Task, mode: AsyncMode)
    分发闭包模式任务：私有泛型函数，由ClosureSystem执行的同步包装闭包调用
    约束：
        T: Send + Sync + 'static
        E: From<AsyncTaskError> + Send + Sync + 'static
        Task: AsyncTask<T, E, Args>
    行为：
        从world取得RuntimeHandle、RuntimeEventSender和AsyncRuntimeHandle
        调用World::emit_pending创建EventHandle<Result<T, E>>
        mode为Await时调用RuntimeHandle::close_gate
        使用RuntimeEventSender构造AsyncContext
        构造在异步线程中调用task.run(context)的Future
        调用submit_supervised提交Future、pending句柄、mode和RuntimeHandle

submit_supervised<T, E, TaskFuture>(executor: &AsyncRuntimeHandle, runtime: RuntimeHandle, handle: EventHandle<Result<T, E>>, mode: AsyncMode, future: TaskFuture)
    提交受监督任务：私有泛型函数，统一处理两种模式的任务完成、panic、取消、唤醒和阀
    约束：
        T: Send + Sync + 'static
        E: From<AsyncTaskError> + Send + Sync + 'static
        TaskFuture: Future<Output = Result<T, E>> + Send + 'static
    行为：
        构造监督Future
            在内层tokio::spawn运行future并await JoinHandle
            正常结束时保留业务Result<T, E>
            panic或取消时转换为AsyncTaskError再转换为E
            调用EventHandle::complete升变pending响应
            mode为Async时调用RuntimeHandle::wake
            mode为Await时调用RuntimeHandle::open_gate
        擦除监督Future并提交AsyncRuntimeHandle

run_executor(mut task_receiver: UnboundedReceiver<ErasedExecutionTask>, startup_sender: SyncSender<Result<(), io::Error>>)
    运行异步线程：私有函数，task_receiver接收监督Future，startup_sender报告初始化结果
    行为：创建Tokio current-thread Runtime；报告结果；持续接收任务并逐个tokio::spawn

panic_message(payload: Box<dyn Any + Send + 'static>) -> String
    提取panic信息：私有函数，payload是类型擦除panic载荷
    行为：依次尝试String和&'static str，均不匹配时返回固定描述
```

## 持有关系

```text
App
├── Schedule
│   ├── AsyncEventSystem<Request, T, E, Handler, Args>--事件模式，每个Request类型最多一个
│   │   └── Arc<Handler>
│   └── ClosureSystem--由ClosurePlugin持有，统一执行同步闭包与异步分发包装闭包
└── World
    ├── RuntimeHandle Resource
    ├── AsyncRegistry Resource
    │   └── event_systems
    └── AsyncRuntimeHandle Resource
        ├── Option<UnboundedSender<ErasedExecutionTask>>
        └── Option<JoinHandle<()>>

事件模式请求
└── AsyncEventRequest<Request>
    ├── Mutex<Option<Request>>
    └── AsyncMode

闭包模式请求--由ClosurePlugin持有
└── ClosureRequest
    ├── target_schedule
    └── 同步分发闭包
        ├── Task
        └── AsyncMode

专用异步线程
└── Tokio current-thread Runtime
    └── UnboundedReceiver<ErasedExecutionTask>

已提交的受监督任务
├── 业务Future
├── AsyncContext
│   └── RuntimeEventSender
├── EventHandle<Result<T, E>>
├── AsyncMode
└── RuntimeHandle
```

## 逻辑

```text
启动异步基础设施：
    app.add_plugin(RuntimePlugin)
        -> app.add_plugin(AsyncRuntimePlugin)
        -> 创建专用异步线程
        -> 插入AsyncRuntimeHandle
        -> 插入AsyncRegistry
        -> 不自动挂载ClosureSystem

注册事件模式System：
    app.add_async_system(schedule, handler)
        -> 提取Request TypeId
        -> AsyncRegistry检查Request未被注册
        -> 创建持有handler的AsyncEventSystem
        -> 挂载到schedule

发送Async事件模式请求：
    world.send_async_event(event)
        -> AsyncRegistry确认Request已注册
        -> 构造AsyncEventRequest { event, mode: Async }
        -> 调用WorldEventExt::send_event
            -> Core事件入队
            -> RuntimeHandle::wake
        -> 不操作阀
    AsyncEventSystem在所属schedule读取请求
        -> 取出event所有权
        -> 创建pending Result<T, E>
        -> 不关阀
        -> 将event和handler交给异步线程
    任务完成
        -> pending升变为Result<T, E>
        -> 唤醒Runtime

发送Await事件模式请求：
    world.send_await_event(event)
        -> AsyncRegistry确认Request已注册
        -> 构造AsyncEventRequest { event, mode: Await }
        -> 调用WorldEventExt::send_event
            -> Core事件入队
            -> RuntimeHandle::wake
        -> 不操作阀
    AsyncEventSystem在所属schedule读取请求
        -> 取出event所有权
        -> 创建pending Result<T, E>
        -> close_gate
        -> 将event和handler交给异步线程
    当前帧继续执行，下一帧开始前等待
    任务完成
        -> pending升变为Result<T, E>
        -> open_gate
        -> 全部Await任务完成后Runtime被唤醒

挂载统一闭包System：
    app.add_plugin(ClosurePlugin)
        -> app.add_closure_system(schedule)
        -> ClosurePlugin在schedule挂载ClosureSystem
    AsyncRuntimePlugin不再提供或挂载独立的异步闭包转发System

提交Async闭包模式请求：
    world.send_async_closure(schedule, task)
        -> 构造捕获task和AsyncMode::Async的同步分发闭包
        -> 调用WorldClosureExt::send_closure(schedule, wrapper)
        -> ClosurePlugin构造ClosureRequest
        -> RuntimePlugin发送事件并唤醒Runtime
        -> 不操作阀
    目标ClosureSystem取出并执行同步分发闭包
        -> 创建typed pending Result<T, E>
        -> 不关阀
        -> 构造AsyncContext
        -> 将task交给异步线程
    任务完成
        -> pending升变为Result<T, E>
        -> 唤醒Runtime

提交Await闭包模式请求：
    world.send_await_closure(schedule, task)
        -> 构造捕获task和AsyncMode::Await的同步分发闭包
        -> 调用WorldClosureExt::send_closure(schedule, wrapper)
        -> ClosurePlugin构造ClosureRequest
        -> RuntimePlugin发送事件并唤醒Runtime
        -> 不操作阀
    目标ClosureSystem取出并执行同步分发闭包
        -> 创建typed pending Result<T, E>
        -> close_gate
        -> 构造AsyncContext
        -> 将task交给异步线程
    当前帧继续执行，下一帧开始前等待
    任务完成
        -> pending升变为Result<T, E>
        -> open_gate

共享监督逻辑：
    事件模式和闭包模式最终都调用submit_supervised
    每个任务在内层tokio::spawn独立执行
    业务Ok或Err保持原样完成pending
    panic或取消转换为AsyncTaskError，再转换为业务E
    监督Future始终持有pending句柄和RuntimeHandle
    mode为Async时完成后唤醒Runtime
    mode为Await时完成后开阀

异步任务发送中间事件：
    AsyncRuntime注入AsyncContext
    任务调用send_event或send_event_after任意次数
    每个事件进入Core队列并唤醒Runtime
    Async任务的中间事件可在任务结束前处理
    Await任务的中间事件等待全部阀打开后处理

响应路由：
    两种模式都产生Result<T, E>响应事件
    Result<T, E>的业务响应System由业务Plugin自行挂载并选择Schedule
    AsyncRuntimePlugin不生成请求ID、不识别Agent、不负责同类型并发响应路由
    开发者自行在T、E或业务类型中携带上下文

错误边界：
    send_async_event或send_await_event的Request未注册 -> AsyncSystemNotRegistered
    send_async_closure或send_await_closure的schedule未挂载ClosureSystem -> ClosureError::ClosureSystemNotRegistered
    重复注册Request -> AsyncSystemAlreadyRegistered
    同schedule重复挂载ClosureSystem -> ClosureError::ClosureSystemAlreadyRegistered
    业务错误由开发者定义E，也可选择anyhow::Error
    AsyncTaskError通过E::from进入Result响应，AsyncRuntimePlugin不依赖anyhow
    锁中毒、类型擦除不一致和内部状态不一致属于不变量破坏，直接panic

Schedule边界：
    add_async_system和add_closure_system只能在App启动前调用
    send_async_closure和send_await_closure只选择已挂载的ClosureSystem，绝不在运行时修改Schedule
    请求在发送后的下一次tick进入读取存储，再由目标System处理
    单次Schedule只能处理它执行前已经进入读取存储的请求，其执行后不再接受新请求
```

## 职责

```text
AsyncRuntimePlugin：
    启动并管理专用异步线程
    提供事件模式与闭包模式
    提供并共享pending响应、任务监督、panic转换、唤醒和阀逻辑
    不定义也不自动挂载闭包转发System

事件模式System：
    持有固定异步处理闭包
    读取固定Request类型
    创建pending响应
    根据Async或Await决定是否关阀
    提交异步任务

ClosurePlugin与ClosureSystem：
    由开发者显式挂载到允许处理一次性闭包的Schedule
    接收AsyncRuntimePlugin生成的同步分发闭包
    不认识异步任务、AsyncContext、pending事件或Runtime阀

异步分发包装闭包：
    由send_async_closure或send_await_closure创建
    被ClosureSystem传入&mut World并同步调用
    创建pending响应
    根据Async或Await决定是否关阀
    构造并注入AsyncContext
    提交异步任务

RuntimePlugin：
    不执行异步任务
    不创建或填充pending事件
    只提供运行循环、RuntimeHandle、阀与事件发送扩展
```
