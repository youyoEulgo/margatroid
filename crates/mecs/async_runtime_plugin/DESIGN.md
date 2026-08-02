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
    RequestAlreadyRegistered {
        request_type: &'static str--请求类型名
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

AsyncTask<T, E, Args>：异步任务适配层，公开trait--进入AsyncRequest后擦除具体任务类型
    Future: Future<Output = Result<T, E>> + Send + 'static--关联类型
    run(self, context: AsyncContext) -> Self::Future
        运行任务：公开方法，context由AsyncRuntime注入
    impl<T, E, Task, TaskFuture> AsyncTask<T, E, ()> for Task
        无上下文函数：公开泛型trait实现
        约束：
            Task: FnOnce() -> TaskFuture + Send + 'static
            TaskFuture: Future<Output = Result<T, E>> + Send + 'static
        run(self, _context: AsyncContext) -> Self::Future
            运行任务：忽略注入的上下文
            行为：调用self()
    impl<T, E, Task, TaskFuture> AsyncTask<T, E, (AsyncContext,)> for Task
        上下文函数：公开泛型trait实现
        约束：
            Task: FnOnce(AsyncContext) -> TaskFuture + Send + 'static
            TaskFuture: Future<Output = Result<T, E>> + Send + 'static
        run(self, context: AsyncContext) -> Self::Future
            运行任务：把context传入业务函数
            行为：调用self(context)

AsyncRequestMode：异步请求模式，公开枚举
    Normal--普通，任务完成后唤醒Runtime
    BlockNextFrame--阻塞下一帧，任务开始前关阀，完成后开阀

AsyncRequest<T, E>：异步请求，公开结构体--本身是正常事件，持有只能取出一次的异步任务
    task: Mutex<Option<ErasedAsyncTask<T, E>>>--类型擦除任务，私有
    mode: AsyncRequestMode--请求模式，私有
    new<Task, Args>(task: Task) -> Self
        构造普通请求：公开泛型关联函数，task是需要执行的业务异步任务
        约束：Task: AsyncTask<T, E, Args>
        行为：调用from_task并使用Normal
    blocking<Task, Args>(task: Task) -> Self
        构造阻塞请求：公开泛型关联函数，task是需要执行的业务异步任务
        约束：Task: AsyncTask<T, E, Args>
        行为：调用from_task并使用BlockNextFrame
    from_task<Task, Args>(task: Task, mode: AsyncRequestMode) -> Self
        按模式构造：私有泛型关联函数，mode指定请求模式
        约束：Task: AsyncTask<T, E, Args>
        行为：将task包装为接收AsyncContext的ErasedAsyncTask并保存
    take_task(&self) -> ErasedAsyncTask<T, E>
        取出任务：crate公开方法
        行为：锁定task并取出所有权；已取出时终止并报告只能执行一次
    mode(&self) -> AsyncRequestMode
        获取请求模式：crate公开方法
        行为：返回mode
    impl<T, E> Event for AsyncRequest<T, E>
        Event：公开泛型trait实现
        约束：
            T: Send + Sync + 'static
            E: From<AsyncTaskError> + Send + Sync + 'static

AsyncRuntimePlugin：异步Runtime插件，公开单元结构体
    impl Plugin for AsyncRuntimePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装专用异步执行器与请求注册表
            行为：
                RuntimeHandle不存在时报告RuntimePluginMissing
                AsyncRuntimeHandle或AsyncRequestRegistry已存在时报告AsyncRuntimePluginAlreadyInstalled
                调用start_executor并插入AsyncRuntimeHandle
                创建并插入AsyncRequestRegistry

AppAsyncExt：App异步扩展，公开trait--挂载类型化异步请求分发System
    add_async_system<T, E>(&mut self, schedule: &str) -> &mut Self
        添加异步System：公开泛型方法，schedule指定分发阶段
        约束：
            T: Send + Sync + 'static
            E: From<AsyncTaskError> + Send + Sync + 'static
        行为：注册AsyncRequest<T, E>；重复时报告RequestAlreadyRegistered；向schedule添加dispatch_async_requests<T, E>
    impl AppAsyncExt for App
        AppAsyncExt for App：公开trait实现
        add_async_system<T, E>(&mut self, schedule: &str) -> &mut Self
            添加异步System：按trait定义执行
            约束：
                T: Send + Sync + 'static
                E: From<AsyncTaskError> + Send + Sync + 'static
            行为：Core在事件首次到期时自动建立AsyncRequest<T, E>与Result<T, E>读取存储

WorldAsyncExt：World异步扩展，公开trait--发送异步请求或启动长期异步服务
    send_async_event<T, E, Task, Args>(&self, task: Task, blocking: bool)
        发送异步事件：公开泛型方法，task是业务任务，blocking决定是否阻塞下一帧
        约束：
            T: Send + Sync + 'static
            E: From<AsyncTaskError> + Send + Sync + 'static
            Task: AsyncTask<T, E, Args>
        行为：按blocking构造AsyncRequest::new或blocking，并调用WorldEventExt::send_event
    spawn_async_service<Service>(&self, service: Service)
        启动长期异步服务：公开泛型方法，service由所属Plugin自行管理生命周期
        约束：Service: Future<Output = ()> + Send + 'static
        行为：从World取得AsyncRuntimeHandle并提交擦除后的Future；不创建请求、pending事件或响应
    impl WorldAsyncExt for World
        WorldAsyncExt for World：公开trait实现
        send_async_event<T, E, Task, Args>(&self, task: Task, blocking: bool)
            发送异步事件：按trait定义执行
            约束：
                T: Send + Sync + 'static
                E: From<AsyncTaskError> + Send + Sync + 'static
                Task: AsyncTask<T, E, Args>
            行为：构造请求并发送到Runtime
        spawn_async_service<Service>(&self, service: Service)
            启动长期异步服务：按trait定义执行
            约束：Service: Future<Output = ()> + Send + 'static
            行为：擦除并提交service
```

crate公开：
```text
ErasedFuture<T, E>：类型擦除Future，crate公开类型别名--等于Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'static>>

ErasedAsyncTask<T, E>：类型擦除异步任务，crate公开类型别名--等于Box<dyn FnOnce(AsyncContext) -> ErasedFuture<T, E> + Send + 'static>

ErasedExecutionTask：类型擦除执行任务，crate公开类型别名--等于Pin<Box<dyn Future<Output = ()> + Send + 'static>>

AsyncRequestRegistry：异步请求注册表，crate公开结构体--防止同一种请求挂载多个分发System
    registered: HashSet<TypeId>--已注册请求，私有
    new() -> Self
        构造注册表：crate公开关联函数
        行为：构造空HashSet
    register<T, E>(&mut self) -> bool
        注册请求：crate公开泛型方法
        约束：
            T: Send + Sync + 'static
            E: From<AsyncTaskError> + Send + Sync + 'static
        行为：插入AsyncRequest<T, E>的TypeId，首次返回true，已存在返回false
    impl Resource for AsyncRequestRegistry
        Resource：crate公开trait实现
```

## System

私有：
```text
dispatch_async_requests<T, E>(world: &mut World)
    分发异步请求：私有泛型System，world提供请求读取器、pending事件和执行器
    约束：
        T: Send + Sync + 'static
        E: From<AsyncTaskError> + Send + Sync + 'static
    行为：
        读取本次更新的全部AsyncRequest<T, E>并取出task与mode
        取得RuntimeHandle、RuntimeEventSender和AsyncRuntimeHandle
        逐个请求创建EventHandle<Result<T, E>>
        BlockNextFrame时调用close_gate
        使用RuntimeEventSender构造AsyncContext
        构造监督Future，在内层tokio::spawn运行task并await JoinHandle
        正常结束保留业务Result；panic或取消转换为AsyncTaskError再转换为E
        调用EventHandle::complete升变响应事件
        Normal调用wake，BlockNextFrame调用open_gate
        向AsyncRuntimeHandle提交监督Future
```

## 函数

crate公开：
```text
start_executor() -> AsyncRuntimeHandle
    启动异步执行器：crate公开函数
    行为：创建任务和启动确认通道，启动专用线程，确认Tokio Runtime创建成功后返回AsyncRuntimeHandle
```

私有：
```text
run_executor(mut task_receiver: UnboundedReceiver<ErasedExecutionTask>, startup_sender: SyncSender<Result<(), io::Error>>)
    运行异步线程：私有函数，task_receiver接收监督Future，startup_sender报告初始化结果
    行为：创建Tokio current-thread Runtime；报告结果；持续接收任务并逐个tokio::spawn

panic_message(payload: Box<dyn Any + Send + 'static>) -> String
    提取panic信息：私有函数，payload是类型擦除panic载荷
    行为：依次尝试String和&'static str，均不匹配时返回固定描述
```

## 持有关系

```text
配置完成：
App
└── World
    ├── RuntimeHandle Resource
    ├── AsyncRequestRegistry Resource
    └── AsyncRuntimeHandle Resource
        ├── Option<UnboundedSender<ErasedExecutionTask>>
        └── Option<JoinHandle<()>>

专用异步线程
└── Tokio current-thread Runtime
    └── UnboundedReceiver<ErasedExecutionTask>

已提交的受监督任务
├── ErasedAsyncTask<T, E>
├── AsyncContext
│   └── RuntimeEventSender
├── EventHandle<Result<T, E>>
└── RuntimeHandle克隆
```

## 逻辑

```text
启动异步线程：
    -> start_executor创建专用线程
    -> run_executor创建Tokio current-thread Runtime
    -> 通过启动确认通道报告成功或ExecutorRuntimeBuildFailed
    -> 持续await task_receiver
    -> 每收到一个ErasedExecutionTask，单独tokio::spawn

添加异步System：
    app.add_async_system<T, E>(schedule)
        -> AsyncRequestRegistry::register<T, E>
        -> 重复时报告RequestAlreadyRegistered
        -> 向schedule添加dispatch_async_requests<T, E>

发送普通异步请求：
    world.send_async_event(task, false)
        -> 构造AsyncRequest::new(task)
        -> 发送事件并唤醒Runtime
        -> dispatch_async_requests创建pending Result<T, E>
        -> 异步线程执行任务
        -> 任务完成后EventHandle::complete
        -> RuntimeHandle::wake
        -> 下一帧读取Result<T, E>

发送阻塞下一帧的异步请求：
    world.send_async_event(task, true)
        -> 构造AsyncRequest::blocking(task)
        -> 分发时RuntimeHandle::close_gate
        -> 当前帧完成，下一帧前等待
        -> 任务完成并升变响应事件
        -> RuntimeHandle::open_gate
        -> 所有阻塞计数归零时唤醒Runtime

异步任务发送中间事件：
    -> AsyncRuntime注入AsyncContext
    -> 任务调用send_event或send_event_after任意次数
    -> 每个事件进入Core队列并唤醒Runtime
    -> Normal请求的中间事件可在任务结束前处理
    -> BlockNextFrame请求的事件等待开阀后处理

异步任务panic或取消：
    -> 内层JoinHandle返回错误
    -> 监督Future仍持有EventHandle
    -> 转换为AsyncTaskError，再通过E::from转换
    -> 使用Err(E)完成pending响应
    -> 按mode唤醒或开阀

响应路由：
    add_async_system只挂载读取AsyncRequest<T, E>的通用分发System
    Result<T, E>业务响应System由业务Plugin自行挂载并选择Schedule
    AsyncRuntimePlugin不生成请求ID、不识别Agent、不负责同类型并发请求路由
    开发者自行在T、E或业务类型中携带上下文

错误边界：
    业务错误由开发者定义E，也可选择anyhow::Error
    AsyncTaskError通过E::from进入Result响应，AsyncRuntimePlugin不依赖anyhow
    AsyncRuntimeError描述Plugin配置与执行器基础设施错误
    锁中毒、类型擦除不一致和内部状态不一致属于不变量破坏，直接panic
```

## 职责

```text
AsyncRuntimePlugin：
    启动并管理专用异步线程
    提供AsyncRequest与泛型分发System
    创建pending响应事件
    监督正常完成、业务错误、panic与取消
    根据请求模式唤醒Runtime或控制阀

RuntimePlugin：
    不执行异步任务
    不创建或填充pending事件
    只提供运行循环、RuntimeHandle、阀与事件发送扩展
```
