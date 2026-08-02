# AsyncRuntimePlugin

## 类型

公开：
```text
异步请求模式：枚举
    普通--任务完成后唤醒Runtime
    阻塞下一帧--任务开始前关闭一层阀，完成后打开一层阀并唤醒Runtime

AsyncTaskError：枚举--可以通过From转换进入开发者选择的响应错误类型E
    NonExhaustive：公开属性--允许后续版本增加错误变体
    Panicked { message: 字符串 }
    Cancelled
    从JoinError构造：crate公开方法
        签名：from_join_error(error: JoinError) -> Self
        行为：JoinError表示panic时提取panic信息构造Panicked，否则构造Cancelled
    Debug：公开trait实现
        签名：Debug for AsyncTaskError
    Display：公开trait实现
        签名：Display for AsyncTaskError
        行为：输出任务panic信息或任务已取消
    Error：公开trait实现
        签名：Error for AsyncTaskError

AsyncRuntimeError：枚举--描述异步执行基础设施的配置错误与运行错误
    NonExhaustive：公开属性--允许后续版本增加错误变体
    RuntimePluginMissing
    AsyncRuntimePluginMissing
    AsyncRuntimePluginAlreadyInstalled
    RequestAlreadyRegistered { request_type: 静态字符串引用 }
    ExecutorThreadStartFailed { source: IO错误 }
    ExecutorRuntimeBuildFailed { source: IO错误 }
    ExecutorDisconnected
    终止：crate公开方法
        签名：panic(self) -> Never
        行为：使用自身Display描述触发panic
    Debug：公开trait实现
        签名：Debug for AsyncRuntimeError
    Display：公开trait实现
        签名：Display for AsyncRuntimeError
        行为：输出包含错误类型与上下文的稳定错误描述
    Error：公开trait实现
        签名：Error for AsyncRuntimeError
        行为：ExecutorThreadStartFailed与ExecutorRuntimeBuildFailed返回内部IO错误作为source，其他变体不返回source

AsyncRuntimeHandle：结构体--由World持有，只通过受控拓展接口提交异步任务
    任务发送端：可选<异步任务发送端>--私有
    异步线程：可选<线程句柄>--私有
    Resource：公开trait实现
    构造：crate公开方法
        签名：new(任务发送端, 异步线程) -> Self
        行为：将任务发送端和异步线程分别包装为可选并构造AsyncRuntimeHandle
    提交任务：crate公开方法
        签名：spawn(&self, task: 类型擦除执行任务)
        行为：将task发送到异步线程，通道断开时终止并报告AsyncRuntimeError::ExecutorDisconnected
    Drop：私有trait实现
        签名：Drop for AsyncRuntimeHandle
        行为：先丢弃任务发送端关闭通道，再join专用异步线程并取消仍在运行的长期服务

异步上下文：结构体--由AsyncRuntime自动注入异步任务
    Runtime事件发送器：Runtime事件发送器--私有
    Clone：公开trait实现
    发送事件：公开泛型方法
        签名：send_event<事件类型:Event>(&self, event: 事件类型)
        行为：调用Runtime事件发送器::send_event，事件入队后唤醒Runtime
    延迟发送事件：公开泛型方法
        签名：send_event_after<事件类型:Event>(&self, event: 事件类型, delay: 无符号整数)
        行为：调用Runtime事件发送器::send_event_after，事件入队后唤醒Runtime

异步任务<T, E, Args>：trait--公开泛型适配层，进入异步请求后擦除具体任务类型
    Future：关联类型，满足Future<Output = Result<T, E>> + Send + 'static
    运行：公开方法
        签名：run(self, context: 异步上下文) -> Future
    无上下文函数：公开trait实现
        约束：Task满足FnOnce() -> Future + Send + 'static
        行为：忽略AsyncRuntime注入的异步上下文并调用Task
    上下文函数：公开trait实现
        约束：Task满足FnOnce(异步上下文) -> Future + Send + 'static
        行为：将AsyncRuntime注入的异步上下文传给Task

异步请求<T, E>：结构体--本身是正常事件，持有只能取出一次的异步任务
    异步任务：互斥锁<可选<类型擦除异步任务<T, E>>>--私有
    请求模式：异步请求模式--私有
    构造普通请求：公开泛型方法
        签名：new<Task, Args>(task: Task) -> Self
        约束：
            Task: 异步任务<T, E, Args>
            T满足Send + Sync + 'static
            E满足From<AsyncTaskError> + Send + Sync + 'static
        行为：擦除task类型并构造普通异步请求
    构造阻塞请求：公开泛型方法
        签名：blocking<Task, Future>(task: Task) -> Self
        约束：与new相同
        行为：擦除task类型并构造阻塞下一帧的异步请求
    Event：公开trait实现
        约束：T与E满足Send + Sync + 'static

AsyncRuntimePlugin：结构体
    默认：公开trait实现
        签名：Default for AsyncRuntimePlugin
        行为：构造AsyncRuntimePlugin
    Plugin：公开trait实现
        签名：build(self, app: &mut App)
        行为：
            从World获取RuntimeHandle
            如果RuntimeHandle不存在，终止并报告AsyncRuntimeError::RuntimePluginMissing
            如果AsyncRuntimeHandle或异步请求注册表已经存在，终止并报告AsyncRuntimeError::AsyncRuntimePluginAlreadyInstalled
            创建异步任务通道
            创建异步线程启动确认通道
            启动一条专用异步线程并移入任务接收端与启动确认发送端
            如果线程创建失败，终止并报告AsyncRuntimeError::ExecutorThreadStartFailed
            等待异步线程报告Tokio Runtime创建结果
            如果Tokio Runtime创建失败，终止并报告AsyncRuntimeError::ExecutorRuntimeBuildFailed
            使用任务发送端构造AsyncRuntimeHandle并作为Resource插入World
            创建异步请求注册表并作为Resource插入World

App异步拓展：trait
    添加异步System：公开泛型方法
        签名：add_async_system<T, E>(&mut self, schedule: &str) -> &mut Self
        约束：
            T满足Send + Sync + 'static
            E满足From<AsyncTaskError> + Send + Sync + 'static
        行为：
            如果异步请求注册表不存在，终止并报告AsyncRuntimeError::AsyncRuntimePluginMissing
            如果异步请求<T, E>已经注册，终止并报告AsyncRuntimeError::RequestAlreadyRegistered
            向指定Schedule添加异步分发System<T, E>
            AsyncRequest<T, E>与Result<T, E>由Core在事件首次到期时自动建立读取存储
            返回App可变引用

World异步拓展：trait
    发送异步事件：公开泛型方法
        签名：send_async_event<T, E, Task, Args>(&self, task: Task, blocking: 布尔值)
        约束：
            T满足Send + Sync + 'static
            E满足From<AsyncTaskError> + Send + Sync + 'static
            Task满足异步任务<T, E, Args>
        行为：
            blocking为假时使用AsyncRequest::new构造普通请求
            blocking为真时使用AsyncRequest::blocking构造阻塞下一帧的请求
            调用WorldEventExt::send_event发送构造后的AsyncRequest<T, E>并唤醒Runtime
    启动长期异步服务：公开泛型方法
        签名：spawn_async_service<Service>(&self, service: Service)
        约束：Service满足Future<Output = ()> + Send + 'static
        行为：
            AsyncRuntimeHandle不存在时终止并报告AsyncRuntimeError::AsyncRuntimePluginMissing
            擦除service Future类型并提交到异步线程
            不创建AsyncRequest和pending事件
            不操作Runtime阀
            不自动生成完成响应事件
            服务错误、停止与优雅关闭由所属Plugin通过自己的Event、Resource与Handle管理
```

私有：
```text
类型擦除Future<T, E>：类型别名<Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'static>>>

类型擦除异步任务<T, E>：类型别名<Box<dyn FnOnce(异步上下文) -> 类型擦除Future<T, E> + Send + 'static>>

类型擦除执行任务：类型别名<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>

异步请求注册表：结构体--防止同一种异步请求注册多个分发System
    已注册请求：集合<TypeId>
    Resource：crate公开trait实现
    构造：crate公开方法
        签名：new() -> Self
        行为：构造不包含任何TypeId的异步请求注册表
    注册：私有泛型方法
        签名：register<T, E>() -> 布尔值
        行为：插入异步请求<T, E>的TypeId，首次插入返回真，已经存在返回假

异步请求<T, E>：实现
    按模式构造：私有泛型方法
        签名：from_task<Task, Future>(task: Task, mode: 异步请求模式) -> Self
        约束：与new相同
        行为：擦除task类型并使用指定请求模式构造异步请求
    取出任务：crate公开方法
        签名：take_task(&self) -> 类型擦除异步任务<T, E>
        行为：
            锁定异步任务
            取出任务所有权
            如果任务已经被取出，终止并报告异步请求只能执行一次
    获取请求模式：crate公开方法
        签名：mode(&self) -> 异步请求模式
        行为：返回请求模式
```

## 持有关系

```text
配置完成：
App
└── World
    ├── RuntimeHandle资源
    ├── 异步请求注册表资源
    └── AsyncRuntimeHandle资源
        └── 异步任务发送端

专用异步线程
└── Tokio单线程Runtime
    └── 异步任务接收端

已提交的异步任务
├── 类型擦除异步任务<T, E>
├── 异步上下文
│   └── Runtime事件发送器
├── 事件句柄<Result<T, E>>
└── RuntimeHandle克隆
```

## System

私有：
```text
异步分发System<T, E>：泛型System
    签名：dispatch_async_requests<T, E>(world: &mut World)
    约束：
        T满足Send + Sync + 'static
        E满足From<AsyncTaskError> + Send + Sync + 'static
    行为：
        读取本次更新的全部异步请求<T, E>事件
        从World借用AsyncRuntimeHandle
        从World取得RuntimeHandle克隆
        从World取得Runtime事件发送器
        循环：逐个处理异步请求
            从异步请求取出任务所有权
            调用World::emit_pending创建pending响应事件并取得事件句柄<Result<T, E>>
            如果请求模式为阻塞下一帧，调用RuntimeHandle::close_gate
            使用Runtime事件发送器克隆构造异步上下文
            构造持有事件句柄的任务监督Future并擦除类型
                将异步上下文传给类型擦除异步任务
                将异步任务放入内层Tokio任务并spawn，任务监督Future保留事件句柄
                await内层任务的JoinHandle
                分流：JoinHandle结果
                    正常完成：取得异步任务返回的Result<T, E>
                    任务panic：提取panic信息，构造AsyncTaskError::Panicked并转换为E
                    任务取消：构造AsyncTaskError::Cancelled并转换为E
                调用事件句柄::complete将最终Result<T, E>升变为正常响应事件
                分流：请求模式
                    普通：调用RuntimeHandle::wake
                    阻塞下一帧：调用RuntimeHandle::open_gate
            调用AsyncRuntimeHandle::spawn提交类型擦除执行任务
```

## 函数

私有：
```text
启动异步执行器：私有函数
    签名：start_executor() -> AsyncRuntimeHandle
    行为：创建任务通道和启动确认通道，启动专用线程，确认Tokio Runtime创建成功后返回AsyncRuntimeHandle

运行异步线程：私有函数
    签名：run_executor(任务接收端, 启动确认发送端)
    行为：创建Tokio current-thread Runtime，报告启动结果，并持续接收任务监督Future后逐个tokio::spawn

提取panic信息：私有函数
    签名：panic_message(payload: 类型擦除panic载荷) -> 字符串
    行为：依次尝试读取String和静态字符串引用，均不匹配时返回固定的非字符串panic描述
```

## 逻辑

```text
启动异步线程：
    -> 专用线程创建Tokio current-thread Runtime
    -> 通过启动确认通道向Plugin报告创建成功或AsyncRuntimeError::ExecutorRuntimeBuildFailed
    -> 在Runtime中持续await异步任务接收端
    -> 每收到一个任务监督Future，单独调用tokio::spawn
    -> 所有任务在同一条异步线程并发执行

添加异步System：
    app.add_async_system<T, E>(schedule)
        -> 在异步请求注册表中注册异步请求<T, E>
        -> 已经注册时终止并报告AsyncRuntimeError::RequestAlreadyRegistered
        -> 向指定Schedule添加异步分发System<T, E>
        --AsyncRequest<T, E>与Result<T, E>均由Core在事件首次到期时自动建立读取存储

发送异步事件：
    world.send_async_event(task, blocking)
        -> 根据blocking构造普通或阻塞AsyncRequest<T, E>
        -> 调用WorldEventExt::send_event发送请求并唤醒Runtime

发送普通异步请求：
    -> System发送异步请求::new(task)
    -> 异步分发System读取请求
    -> 创建pending响应事件
    -> 将任务提交到异步线程
    -> Runtime发现只有pending事件时进入等待
    -> 异步任务完成
    -> pending事件升变为延迟0的正常Result<T, E>事件
    -> 唤醒Runtime
    -> 下一次tick读取响应事件

异步任务发送中间事件：
    -> AsyncRuntime自动向异步任务注入异步上下文
    -> 异步任务调用异步上下文::send_event一次或多次
    -> 每个事件直接写入Core事件队列并唤醒Runtime
    -> 普通请求的中间事件可以在任务完成前被System处理
    -> 阻塞请求的中间事件先入队，等待任务完成并开阀后处理
    -> 异步任务最终返回Result<T, E>并完成自身pending响应事件

异步任务panic：
    -> 内层Tokio任务panic并返回JoinError
    -> 任务监督Future仍然持有事件句柄
    -> 从JoinError提取panic信息并构造AsyncTaskError::Panicked
    -> 调用E::from转换为开发者选择的响应错误类型
    -> 使用Err(E)完成pending响应事件
    -> 普通请求唤醒Runtime，阻塞请求打开阀
    -> 下一次tick由响应System读取错误响应

发送阻塞下一帧的异步请求：
    -> System发送异步请求::blocking(task)
    -> 异步分发System创建pending响应事件
    -> RuntimeHandle::close_gate使阻塞计数加1
    -> 当前帧继续执行完成
    -> Runtime在下一帧前进入等待
    -> 异步任务完成并升变pending响应事件
    -> RuntimeHandle::open_gate使阻塞计数减1
    -> 所有阻塞任务完成、计数归零时唤醒Runtime
    -> 下一帧读取响应事件

响应事件区分：
    add_async_system只挂载读取AsyncRequest<T, E>的通用异步分发System
    读取Result<T, E>的业务响应System由业务Plugin自行挂载并选择Schedule
    AsyncRuntimePlugin不生成请求ID、不识别Agent，也不负责响应路由
    多个同类型并发请求由开发者自行在T、E或业务类型中携带请求ID、Agent ID或其他上下文
    响应System按照开发者定义的上下文自行完成路由

错误边界：
    业务错误由开发者定义E，或选择anyhow::Error等通用错误类型
    AsyncTaskError通过E::from进入Result<T, E>响应事件，AsyncRuntimePlugin不依赖anyhow
    AsyncRuntimeError描述Plugin配置与执行器基础设施错误，遇到时终止并报告对应错误
    锁中毒、类型擦除不一致和内部状态不一致属于不变量破坏，直接panic且不进入公开错误类型
```

## 职责

```text
AsyncRuntimePlugin：
    持有异步执行器的任务发送端
    启动并管理专用异步线程
    提供异步请求事件与泛型分发System
    创建pending响应事件
    监督异步任务的正常完成、业务错误、panic与取消
    无论任务如何结束都将pending事件升变为Result<T, E>响应事件
    根据请求模式唤醒Runtime或控制阻塞计数

RuntimePlugin：
    不执行异步任务
    不创建或填充pending事件
    只提供运行循环、阻塞计数、RuntimeHandle与事件发送拓展
```
