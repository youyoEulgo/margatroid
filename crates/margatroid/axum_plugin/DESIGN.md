# 伪代码格式

```text
板块使用一级标题，一行注明文件；类型、函数使用二级标题，列出字段、签名及直接行为。
Resource在lib板块；System区分事件、闭包结果、提交闭包及回复。
持有关系使用树形图，无内容不建立空模块。
```

# 设计范围

本文定义Margatroid的Axum基础设施实现。各领域使用原生Router及普通异步闭包；mecs仅提供通用运行设施，不提供服务器和长期任务专用接口。不增加旧网络桥兼容层，不改变现有外部业务协议，不实现Driver WebUI。

# 板块格式

```text
# lib     axum_plugin/src/lib.rs      插件安装、路由注册与共享生命周期
# system  axum_plugin/src/system.rs   服务启动及异步闭包结果收口
# handler axum_plugin/src/handler.rs  Axum监听、停止及真实退出
# types   axum_plugin/src/types.rs    服务材料与结果
# error   axum_plugin/src/error.rs    服务错误
```

# lib

## 类型

公开：
```text
AxumPlugin：Margatroid的Axum服务装配基础设施，不属于业务领域plugin
    bind: SocketAddr
    impl Plugin
        build(self, app: &mut App)
            行为：检查既有Runtime、AsyncRuntime与Closure设施；注册空AxumRoutes、AxumHandle及启动System；不注册任何业务路由、协议分类器或通用请求事件。

AppAxumExt：原生路由装配入口
    add_axum_routes(&mut self, router: axum::Router) -> &mut Self
        行为：启动前合并已完成with_state的Router<()>；handler、extractor、middleware及错误响应由提供该Router的插件决定；启动后拒绝变更，重复路由由Axum装配校验拒绝。
    impl AppAxumExt for App

AxumRoutes：装配阶段Router资源
    router: Option<axum::Router>--启动时取走，之后为None
    impl Resource

AxumLifetime：可克隆服务生命周期能力
    shutdown: CancellationToken--共享停止信号
    connections: TaskTracker--已升级连接的退出跟踪，不接管连接业务
    cancelled(&self) -> Future<Output = ()>
        行为：handler可选择监听共享停止信号，停止接收新业务并关闭本插件连接。
    track(&self) -> AxumWorkGuard
        行为：取得一次升级工作的RAII退出登记；不自行spawn、不生成ECS事件、不解析网络数据。WebSocket/SSE等脱离普通HTTP请求寿命的工作在升级前登记，再将guard移动到升级回调，持有到完整连接Future退出；升级失败则释放guard。

AxumWorkGuard：一次已登记升级工作的退出凭证
    行为：包装TaskTracker token；drop只表示持有它的完整工作已退出，不主动终止工作，不代表业务执行成功。必须覆盖读写及子任务join，不能在spawn子任务后提前drop。

AxumHandle：监听状态和停止能力，公开Resource
    lifetime: AxumLifetime
    local_address: Arc<RwLock<Option<SocketAddr>>>
    completion: Arc<服务结果通知状态>
    lifetime(&self) -> AxumLifetime
        行为：克隆共享停止与升级工作跟踪能力，不暴露Router或World。
    request_shutdown(&self)
        行为：只请求停止，不宣称完成。
    wait_stopped(&self) -> Future<Output = Result<(), AxumError>>
        行为：等待监听服务、普通请求和所有已登记升级连接真实退出；错误保留并交付，不以丢弃Future或销毁Resource代替退出证明。
```

## 边界

```text
允许：原生Router合并、绑定端口、服务停止信号、升级工作退出跟踪、最终结果。
不提供：HttpRequestReceived、WebSocketMessageReceived、WebSocketStreamOpened、mecs_stream分类、统一响应Session、Client名称索引或业务请求排队。
插件可在自己的handler中直接Response、自建channel/oneshot、发送所属领域事件或插件专用事件；AxumPlugin不要求任何一种通信方式。
AxumLifetime只协调退出，不能被扩展成另一套长期异步服务执行器或通用消息桥。
业务插件之间的依赖边界保持；DTO和UI依赖本基础设施仅用于原生路由与生命周期装配，不意味着允许业务plugin互相调用。
```

# system

## 函数

crate公开：
```text
start_axum_system(world: &mut World)
    处理事件：None
    接收闭包结果：None
    提交闭包：run_axum(AxumContext) -> Future<Output = Result<AxumStopped, AxumTaskError>>
    产出事件：None
    行为：STARTUP取走Router；取得owned监听参数、AxumHandle和运行通知能力，通过send_async_closure提交整个Axum服务。不得使用send_await_closure关闭ECS运行门，不通过临时握手获取Tokio Handle。

axum_result_system(world: &mut World)
    处理事件：AxumStarted--本封装自己的监听生命周期通知
    接收闭包结果：Result<AxumStopped, AxumTaskError>
    提交闭包：None
    产出事件：None
    行为：更新可观察监听状态及最终结果；bind/serve/panic错误交付AxumHandle和应用诊断。通知只描述服务生命周期，不描述网络请求或业务消息。
```

# handler

## 函数

私有：
```text
run_axum(context: AxumContext) -> Future<Output = Result<AxumStopped, AxumTaskError>>
    行为：
        在普通异步闭包内TcpListener::bind；成功取得真实SocketAddr写入共享句柄并发送AxumStarted，不用阻塞握手获取执行器。
        将AxumLifetime作为Extension注入Router，运行axum::serve(...).with_graceful_shutdown(...)。
        收到停止请求或serve错误时取消共享shutdown；各插件handler自行响应并关闭连接。
        停止接收后等待普通HTTP请求和升级注册收口；关闭connections tracker并等待已登记Future全部退出，只有此后确认AxumStopped。
        Axum升级回调不会自动受HTTP graceful shutdown等待，因此guard必须在on_upgrade注册前建立，再移入回调；socket在回调时才可用，不在升级前调用需要socket的serve_connection。握手失败释放预登记guard，避免关闭时遗漏或永久等待。
        不添加固定超时后直接丢弃未退出连接的行为；无法退出时保留监督与诊断，应用不能报告安全退出。
        即使serve出错也先通知停止并完成所属工作清理，错误交付与真实完成状态分开。
```

# types

## 类型

私有：
```text
AxumContext：owned Router、监听参数、AxumHandle及RuntimeEventSender
AxumStarted：监听地址生命周期通知，插件私有Event
AxumStopped：服务及已登记工作全部退出的完成载荷
AxumTaskError：AxumError及必要原始关联，实现From<AsyncTaskError>
```

# error

## 类型

公开：
```text
AxumError：安装、路由冻结、bind、serve及清理错误
    kind: AxumErrorKind
    message: String
    impl Display + std::error::Error
```

# 持有关系

```text
Margatroid App
├── mecs Runtime + AsyncRuntime + Closure（本阶段保留原实现）
├── AxumPlugin
│   ├── AxumRoutes--只在启动前持有Router
│   └── AxumHandle--停止与真实完成
├── DtoPlugin--提供/ws Router及自有State
└── UiPlugin--提供静态Router

普通异步闭包run_axum
├── TcpListener + 合并后的原生Router
├── graceful shutdown Future
└── AxumLifetime
    ├── shutdown token
    └── 已升级工作退出跟踪
        └── 各插件自己的连接Future、channel或事件适配
```
