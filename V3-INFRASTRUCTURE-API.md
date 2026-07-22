# V3 基础设施公开 API 规范

状态：草案

目标：将 Margatroid 的同步 ECS 内核和通用运行时维护成可以独立测试、独立使用、
最终可以发布到 crates.io 的基础设施 crate 集合。

这套 ECS 与基础设施体系暂定名为 **mecs**。该名称目前用于设计和对外表达，
现有 crate 不因此立即重命名；发布前再统一确认 crates.io 名称可用性与迁移方案。

现阶段文档使用中文；准备公开发布时再补充英文 crate 文档和 i18n。

## 1. 设计原则

基础设施遵循以下原则：

- KISS：core 只保留 ECS 成立所需的最小能力。
- 领域无关：公开 API 不出现 LLM、agent、workspace、workflow 等业务类型。
- 显式组合：运行循环、异步执行、日志分别由独立 Plugin 提供。
- 同步 World：所有 ECS System 都是同步函数，只有主线程修改 World。
- 可预测：Stage 顺序、事件保留、panic 和异步结果回流都有明确语义。
- 可替换：可选基础设施不通过 core feature flag 隐式开启。
- 可审计：公开导出面保持小而稳定，线程和全局状态边界必须写入文档。
- 开发者友好：保持 Rust 生态已有的开发习惯，不为 Plugin 重新发明一套包装 API。
- 默认可用：常见场景只需 `add_plugins(...)`，合理默认值即可启动。
- 渐进配置：零配置、builder 配置和生态原生高级 API 分层提供，简单场景不暴露内部复杂度。

判断能力是否应进入 core：

> 删除该能力后，Entity、Component、Resource、System、Schedule、Event 或 Plugin
> 是否无法成立？

如果答案是否定的，该能力应进入独立 crate 或 Plugin。

## 2. Crate 分层

```text
core_plugin
├── World / Entity / Component / Resource
├── Query
├── synchronous System / Schedule
├── Event / EventReader
├── App::tick()
└── Plugin / PluginGroup

app_runtime_plugin
├── AppRunExt::run()
├── AppControl
└── wait / wake / shutdown

async_runtime_plugin
├── AsyncAppExt
├── managed Tokio worker
├── timeout / cancel / backpressure
└── completion → typed Event

log_plugin（第一版已实现）
├── tracing subscriber installation
├── console / rolling file output
└── optional bounded log stream

http_server_plugin（第一版已实现）
├── Axum router composition
├── HTTP / SSE / WebSocket primitives
├── listener lifecycle
└── graceful shutdown

external_event_plugin（第一版已实现）
├── typed external Event registration
├── cloneable bounded sender
├── AppControl wake integration
└── Stage::First drain into World
```

依赖方向固定为：

```text
app_runtime_plugin   → core_plugin
async_runtime_plugin → core_plugin + app_runtime_plugin API
log_plugin           → core_plugin + tracing ecosystem
http_server_plugin   → core_plugin + app_runtime_plugin + Axum/Tokio
external_event_plugin → core_plugin + optional app_runtime_plugin API
```

core 不得反向依赖任何运行时 Plugin。

## 3. 通用 Public API 规则

每个基础设施 crate 的 `lib.rs` 只能导出：

- 用户构建 App 所需的稳定类型
- Plugin 类型和 Options
- 明确公开的 extension trait
- 必要的 Event、Resource、Handle 和 Error

默认不导出：

- System 函数
- worker、channel、receiver、内部 command
- storage 实现
- Tokio runtime 实例
- tracing subscriber 的具体组合类型
- 只为测试存在的 helper

公开类型必须：

- 有 crate-level 文档和最小示例
- 写清 `Send` / `Sync`、线程和生命周期语义
- 避免 public field，优先构造函数和只读方法
- 不泄漏内部依赖的复杂泛型类型
- 错误不得包含 secret、token 或原始 Authorization header

## 4. core_plugin API

### 4.1 职责

```text
App
├── World
│   ├── Entity / Component / Bundle
│   ├── Resource
│   └── Event / EventReader
├── HashMap<Stage, Schedule>
│   └── Vec<Box<dyn System>>
└── Event maintenance state
```

core 负责：

- Entity 创建、删除、generation 校验和 index 复用
- Component 与 Resource 的类型化存取
- Query / QueryMut / Res / ResMut
- 同步 System 注册、排序和 panic 隔离
- Event 注册、发送、独立 Reader 和按帧清理
- Plugin 安装
- 单次 `App::tick()`

core 不负责：

- Tokio、Future、channel、timeout、cancel
- Condvar、无限运行循环和 shutdown
- tracing subscriber
- 网络、文件轮转、配置加载
- 任何 Margatroid 领域模型

core 的依赖树中不得出现 Tokio。

### 4.2 Stage

```rust
pub enum Stage {
    Startup,
    First,
    Update,
    Last,
}
```

执行顺序：

```text
Startup（只执行一次）
First → Update → Last
Event frame maintenance
```

这四个 Stage 不携带业务语义。业务阶段不得重新加入 core。

### 4.3 App

稳定目标 API：

```rust
pub struct App;

impl App {
    pub fn new() -> Self;

    pub fn add_plugins(&mut self, plugins: impl Into<PluginGroup>) -> &mut Self;
    pub fn add_systems(
        &mut self,
        stage: Stage,
        systems: impl IntoIterator<Item = impl System>,
    ) -> &mut Self;

    pub fn add_resource<R: Resource>(&mut self, resource: R) -> &mut Self;
    pub fn add_event<E: Event>(&mut self) -> &mut Self;
    pub fn event_reader<E: Event>(&self) -> EventReader<E>;

    pub fn world(&self) -> &World;
    pub fn world_mut(&mut self) -> &mut World;
    pub fn schedule_mut(&mut self, stage: Stage) -> &mut Schedule;

    pub fn set_event_retention_frames(&mut self, frames: u64) -> &mut Self;
    pub fn tick(&mut self);
}
```

`App` 不提供 `run()`、`add_async_system()`、网络或日志快捷方法。

### 4.4 World

公开能力：

```text
spawn / despawn
insert / remove / get / get_mut / has
iter / iter_mut
add_resource / resource / resource_mut / remove_resource
send_event / read_events
entity_count / is_alive
```

约束：

- `World` 是 ECS 数据唯一所有者。
- `System::run` 期间通过 `&mut World` 修改数据。
- 异步 Future 不得持有 World 引用。
- Resource 必须满足 `'static + Send + Sync`。

### 4.5 System 与 Schedule

```rust
pub trait System: Send + 'static {
    fn run(&mut self, world: &mut World);
    fn label(&self) -> Option<&'static str>;
    fn before(&self) -> &[&'static str];
    fn after(&self) -> &[&'static str];
}
```

语义：

- 无约束 System 保持注册顺序。
- named System 使用拓扑排序解析 `before` / `after`。
- 重复 label、未知 label 和依赖环返回 ordering error。
- 单个 System panic 被隔离，后续 System 继续执行。
- Schedule 不创建线程，不隐式并行执行。

### 4.6 Event

```rust
pub trait Event: Clone + Send + Sync + 'static {}
```

语义：

- 每种 Event 类型在 World 中有独立队列。
- 每个 `EventReader<E>` 有独立游标。
- Reader 创建前已存在的事件默认不读取。
- 事件按 frame 保留；过期后 Reader 可以查询漏读数。
- 读取返回克隆值，不向外暴露队列锁。

### 4.7 Plugin

```rust
pub trait Plugin {
    fn build(&self, app: &mut App);
}
```

`build` 是唯一安装入口，可注册 Resource、Event 和 System。构造函数必须纯净，不能启动线程或修改全局状态。

`CorePlugin` 类型不存在；`App::new()` 已经创建 core。

## 5. app_runtime_plugin API

### 5.1 职责

- 提供阻塞式运行循环
- 无 wake 时阻塞当前线程
- 提供线程安全的 wake / shutdown handle
- 每次 wake 后执行下一次 `App::tick()`

### 5.2 Public API

```rust
pub struct AppRuntimePlugin;

pub trait AppRunExt {
    fn run(&mut self);
}

#[derive(Clone)]
pub struct AppControl;

impl AppControl {
    pub fn wake(&self);
    pub fn shutdown(&self);
    pub fn is_shutdown(&self) -> bool;
}
```

### 5.3 生命周期

- `AppRuntimePlugin::build` 将 `AppControl` 注册为 Resource。
- `run()` 前必须安装 `AppRuntimePlugin`，否则立即报错。
- `run()` 先执行一帧，然后进入阻塞等待。
- wake 具有 pending 语义，先发生的 wake 不会因稍后进入 wait 而丢失。
- shutdown 必须同时唤醒等待线程。

`AppRuntimePlugin` 不定义业务 Stage，也不创建异步任务。

## 6. async_runtime_plugin API

### 6.1 职责

- 将 Request Event 转为独立 Future
- 管理专用 Tokio 工作线程
- 提供有界队列和最大并发限制
- 处理 timeout、cancel、panic 和 worker stop
- 将成功结果作为 Output Event 写回主线程

### 6.2 Public API

```text
AsyncRuntimePlugin
AsyncRuntimeOptions
AsyncAppExt
AsyncSystemOptions
AsyncTaskId
AsyncTaskControl
AsyncWorldExt
AsyncTaskStarted
AsyncTaskFailed
AsyncTaskFailureKind
```

目标扩展 API：

```rust
pub trait AsyncAppExt {
    fn add_async_system<Request, Output, Handler, Fut>(
        &mut self,
        handler: Handler,
    ) -> &mut Self;

    fn add_async_system_with_options<Request, Output, Handler, Fut>(
        &mut self,
        handler: Handler,
        options: AsyncSystemOptions,
    ) -> &mut Self;
}

pub trait AsyncWorldExt {
    fn cancel_async_task(&self, id: AsyncTaskId) -> bool;
}
```

### 6.3 Stage 与数据流

```text
Stage::Startup  启动 managed worker
Stage::First    回收 Completion 并写回 World
Stage::Last     读取 Request Event 并派发 Future
```

```text
Request Event
→ bounded task channel
→ Tokio worker / JoinSet
→ completion channel
→ WorldCommand（主线程执行）
→ Output Event 或 AsyncTaskFailed
```

### 6.4 约束

- 必须先安装 `AsyncRuntimePlugin`，再调用 `AsyncAppExt`。
- Request 与 Output 必须是不同 Event 类型。
- 每个 Request 独立 spawn。
- Future 必须是 `Send + 'static`，不能借用 World。
- 阻塞工作由调用方显式放入 `spawn_blocking`。
- worker Resource Drop 时 shutdown、abort 剩余任务并 join。
- 安装 `AppRuntimePlugin` 时，Completion 会通过 `AppControl` 唤醒主循环。
- 未安装 App runtime 时，测试可以手动调用 `tick()`。

内部 `AsyncWorker`、`AsyncSpawner`、channel 和 `WorldCommand` 不公开。

## 7. log_plugin API

### 7.1 定位

`LogPlugin` 是与 App runtime、Async runtime 同级的基础设施 Plugin，负责为当前进程安装和管理 `tracing` subscriber。

其他 crate 只调用 `tracing::{trace, debug, info, warn, error}`，不得在 Cargo 依赖上依赖 `log_plugin`。CLI、daemon 和测试程序由各自的 composition root 决定是否安装以及如何配置。

`LogPlugin` 只是 `tracing` 的开箱即用配置器，不是新的日志抽象层。
它不定义 `LogEvent`、不包装 tracing 宏，也不改变 Rust 开发者的使用习惯。

```text
CLI process
└── LogPlugin(console, human-readable)

daemon process
└── LogPlugin(console + rolling file, json or compact)
```

### 7.2 全局状态事实

tracing subscriber 是进程级全局状态：

- 一个进程只能成功安装一次 global default subscriber。
- 安装后不能安全卸载或替换。
- Plugin 从 App 中移除不会撤销 subscriber。
- 为捕获其他 Plugin 的 build 日志，初始化不能等到普通 Startup System。

因此 `LogPlugin` 是 install-only、process-scoped Plugin，不承诺运行时卸载。这是公开契约，不隐藏成实现细节。

### 7.3 Public API

公开导出：

```text
LogPlugin
LogOptions
LogLevel
LogFormat
ConsoleOptions
ConsoleTarget
FileLogOptions
LogRotation
LogStreamOptions
LogStream
LogSubscription
LogRecord
LogField
```

不导出：

```text
tracing_subscriber 的组合 subscriber 类型
reload::Handle 的具体泛型
tracing_appender::non_blocking::WorkerGuard
Layer 实现细节
文件清理 worker 和 channel
```

### 7.4 构造 API

```rust
let plugin = LogPlugin::new()
    .with_level(LogLevel::Info)
    .with_format(LogFormat::Compact)
    .with_console(ConsoleOptions::stderr())
    .with_file(
        FileLogOptions::daily("./logs", "margatroidd")
            .with_max_files(14),
    )
    .with_stream(LogStreamOptions::default());
```

也支持完整 Options：

```rust
pub struct LogPlugin {
    // fields are private
}

impl LogPlugin {
    pub fn new() -> Self;
    pub fn with_options(options: LogOptions) -> Self;
    pub fn with_level(self, level: LogLevel) -> Self;
    pub fn with_filter(self, filter: impl Into<String>) -> Self;
    pub fn with_format(self, format: LogFormat) -> Self;
    pub fn with_console(self, options: ConsoleOptions) -> Self;
    pub fn without_console(self) -> Self;
    pub fn with_file(self, options: FileLogOptions) -> Self;
    pub fn with_stream(self, options: LogStreamOptions) -> Self;
}
```

构造函数只保存配置，不安装 subscriber、不打开文件。

`Default` 是稳定公开契约：`Info` 级别、Compact 格式、输出到 stderr，
不开启文件和日志流。因此最小用法只需：

```rust
app.add_plugins(LogPlugin::default());

tracing::info!("application started");
```

### 7.5 配置类型

```rust
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

pub enum LogFormat {
    Compact,
    Pretty,
    Json,
}

pub enum ConsoleTarget {
    Stdout,
    Stderr,
}

pub enum LogRotation {
    Minutely,
    Hourly,
    Daily,
    Never,
}

```

`with_filter()` 使用 `EnvFilter` 语法并覆盖单独的 `LogLevel`。解析失败不会 panic，
而是通过最小 fallback 写 stderr，并回退到 `Info` filter。

`FileLogOptions` 至少包含：

```text
directory
file_name_prefix
rotation
max_files: Option<usize>
non_blocking: bool（默认 true）
```

第一版不支持按文件大小滚动；只支持 tracing-appender 能稳定实现的时间轮转。`max_files` 的清理行为必须在实现前单独测试，不能只写 API 不兑现。

### 7.6 build 与初始化时机

`LogPlugin::build()` 是通用 build 规则的明确例外：它可以安装 global subscriber 和打开日志文件，因为日志必须先于第一个 System 执行。

顺序：

```text
LogPlugin::build
├── 验证 options / filter
├── 构造已启用的 console / file / stream Layer
├── try_init global subscriber
├── 保存 WorkerGuard 到进程级 managed state
└── 启用 stream 时注册 `LogStream` Resource
```

managed state 必须与进程同寿命，不能由某一个 `App` 或 `World` 独占。
`App` Drop 不 Drop 全局 subscriber 所依赖的 WorkerGuard，也不停止全局日志 writer。

composition root 应首先安装日志：

```rust
App::new()
    .add_plugins(LogPlugin::new().with_console(ConsoleOptions::stderr()))
    .add_plugins(AppRuntimePlugin::default())
    .add_plugins(AsyncRuntimePlugin::default());
```

### 7.7 已存在的 subscriber

`LogPlugin` 默认调用 `try_init`：如果进程已有 subscriber，则保留它且不 panic。
由 `LogPlugin` 完成的首次安装会记录进程级 Options；后续相同配置可以在明确请求
stream 时复用同一个 `LogStream`，未请求 stream 的 App 不会得到该 Resource。
后续配置与首次配置不同时，首次配置继续生效，并通过最小 fallback 写 stderr 明确
报告冲突。若 subscriber 由外部代码安装，本次 console、file 和 stream 配置均不生效。
不提供 subscriber 替换或卸载 API。

需要完全自定义 subscriber 或 Layer 的高级用户，应使用 tracing 原生 API 完成安装，
并不安装 `LogPlugin`。`LogPlugin` 不为支持任意 Layer 而向公开 API 泄漏复杂泛型。

### 7.8 Stream Layer

Stream Layer 只负责将 tracing record 写入与传输协议无关的有界广播队列：

```rust
app.add_plugins(
    LogPlugin::default().with_stream(LogStreamOptions::default()),
);

let stream = app.world().resource::<LogStream>();
let subscription = stream.subscribe();
```

`LogRecord` 至少包含 timestamp、level、target、message、structured fields 和 span context。
Layer 使用 Tokio broadcast 的非阻塞 `send`；容量不足时覆盖最旧 record，
慢订阅者在 `recv()` 时收到 `LogStreamError::Lagged(count)`，并可通过
`dropped_count()` 查询自己累计的丢失数。日志线程、其他订阅者和 App 不被慢消费者阻塞。

`LogPlugin` 不开端口、不实现 HTTP/SSE/WebSocket。网络输出由使用
`LogStream` 的上层 Plugin 实现。

### 7.9 诊断日志与 ECS Event

tracing Event 用于诊断，ECS Event 用于驱动程序行为。两者同名但不是同一套机制。
`LogPlugin` 不定义 ECS 日志 Event，也不复制所有 tracing Event 到 ECS Event。

默认禁止实现“每条 tracing 日志转成 ECS Event”，原因是：

- 容易形成日志 → Event → System 日志的递归环
- 增加主线程压力和内存占用
- 与 tracing Layer 的职责重复

对外日志流使用 7.8 的有界 Stream Layer，而不是修改 core Event 系统。

### 7.10 Secret 与安全边界

`LogPlugin` 不可能可靠识别任意业务字段中的 secret，因此不承诺自动脱敏。

硬性规则：

- 业务 Plugin 不得记录 API key、token、Authorization header。
- Debug 输出不得包含完整隐私文件或未裁剪 prompt。
- JSON 格式不等于安全格式。
- 文件权限、目录所有者和日志保留策略由 daemon 配置负责。
- 初始化错误不得回显敏感环境变量值。

### 7.11 测试要求

由于 global subscriber 每进程只能安装一次，测试必须隔离：

- 纯 options/filter 单元测试不安装 global subscriber。
- subscriber 安装和并发重复安装测试使用独立 integration-test binary/process。
- 测试已有 subscriber 时不覆盖且不 panic。
- 测试 console off、JSON、文件创建和轮转。
- 测试 stream 队列满、慢订阅者和 dropped count。
- 测试重复安装不会 panic。
- 除专门验证安装锁的单个隔离测试外，不并行运行共享 global subscriber 的测试。

### 7.12 第一版明确不做

- OpenTelemetry / OTLP
- journald / syslog / Windows Event Log
- 网络日志上传
- 每条日志转 ECS Event
- 基于文件大小轮转
- 跨进程统一日志聚合
- subscriber 替换或卸载
- 运行时 filter reload 和公开 flush handle

这些能力以后通过独立 Layer 或 Plugin 扩展，不扩大 `LogPlugin` 第一版 API。

## 8. http_server_plugin API

### 8.1 定位与边界

`HttpServerPlugin` 是通用 HTTP 服务生命周期 Plugin。它封装 listener、Tokio/Axum server、
Router 合并、SSE/WebSocket 基础能力和 graceful shutdown，但不定义 Margatroid 路由、
业务 Event、用户权限策略或 CLI 协议。

Plugin 保持 Axum 原生开发习惯，不重新发明 Router、Handler 和 Middleware。

### 8.2 Public API 与默认值

```text
HttpServerPlugin
HttpServerOptions
HttpAppExt
HttpServerHandle
HttpServerState
HttpServerStarted
HttpServerFailed
```

默认绑定 `127.0.0.1:3000`，不默认暴露公网端口。最小用法：

```rust
let mut app = App::new();
app.add_plugins(HttpServerPlugin::default());
app.add_http_routes(
    axum::Router::new().route("/health", axum::routing::get(health)),
);
```

常规配置通过 builder 提供：

```rust
HttpServerPlugin::bind("127.0.0.1:8080")
    .with_request_timeout(Duration::from_secs(30))
    .with_max_body_size(8 * 1024 * 1024);
```

### 8.3 路由组合

`HttpAppExt::add_http_routes(Router)` 将 Axum Router 合并到内部 route registry。
`HttpServerPlugin` 必须先安装，依赖它的 Plugin 再注册路由；缺失时在 build 阶段
立即给出明确错误。Server 只在所有 Plugin build 完成后的 Startup 阶段启动，
因此不依赖路由 Plugin 的注册先后来控制请求行为。

### 8.4 日志流协作

`HttpServerPlugin` 不依赖 `log_plugin`。上层适配 Plugin 同时使用 `LogStream`
和 `HttpAppExt`，将订阅编码为 SSE 或 WebSocket：

```text
tracing macros
→ LogPlugin Stream Layer
→ bounded LogStream
→ business server adapter
→ HttpServerPlugin SSE/WebSocket
→ CLI
```

HTTP 端口、路由、鉴权、限流和日志可见性属于上层适配 Plugin，
`LogPlugin` 不自行启动日志 server。

### 8.5 生命周期与测试

- Plugin 构造和 build 不绑定 socket；Startup 阶段才启动 listener。
- 启动失败发布 `HttpServerFailed`，不在 worker 线程中静默退出。
- `AppRuntimePlugin` 提供统一 shutdown 控制；Startup 结果在当前 tick 直接写回 Event。
- App shutdown 时停止接受新连接，等待在途请求到明确 deadline。
- 测试覆盖路由合并、端口冲突、启动失败、SSE 断开和 graceful shutdown。

## 9. external_event_plugin API

### 9.1 定位

`external_event_plugin` 负责将 HTTP handler、文件 watcher、系统信号或其他外部线程
产生的类型化数据，安全地注入只能由主线程修改的 ECS World。

它不将 channel 放入 core，不让外部线程持有 `World` 引用，也不定义 HTTP、CLI
或 Margatroid 业务协议。安装 `AppRuntimePlugin` 时，成功入队后通过
`AppControl::wake()` 唤醒可能正在等待的 App；未安装时仍可手动 `tick()`，
便于测试和嵌入式宿主使用。

### 9.2 Public API

```text
ExternalEventPlugin
ExternalEventAppExt
ExternalEventOptions
ExternalEventSender<E>
ExternalEventSendError<E>
```

最小使用方式：

```rust
let mut app = App::new();
app.add_plugins(AppRuntimePlugin);
app.add_plugins(ExternalEventPlugin::default());
app.add_external_event::<UserPromptSubmitted>();

let sender = app.external_event_sender::<UserPromptSubmitted>();
```

需要自动唤醒时，`AppRuntimePlugin` 必须在调用 `add_external_event::<E>()` 之前安装；
`ExternalEventPlugin` 本身与 `AppRuntimePlugin` 的安装先后不影响该能力。
未安装 App runtime 是受支持的 manual-tick 模式，不是错误。调用 extension API
时若缺失 `ExternalEventPlugin`，则在 build 阶段立即给出明确错误。

### 9.3 注册 API

```rust
pub trait ExternalEventAppExt {
    fn add_external_event<E: Event>(&mut self) -> &mut Self;

    fn add_external_event_with_options<E: Event>(
        &mut self,
        options: ExternalEventOptions,
    ) -> &mut Self;

    fn external_event_sender<E: Event>(&self) -> ExternalEventSender<E>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalEventOptions {
    // private fields
}

impl ExternalEventOptions {
    pub fn new() -> Self;
    pub fn with_capacity(self, capacity: usize) -> Self;
    pub fn with_max_events_per_frame(self, limit: usize) -> Self;
}
```

默认 `capacity = 1024`，`max_events_per_frame = 256`。两者都必须大于 0。

注册 `E` 时同时调用 core 的 `add_event::<E>()`，并在 `Stage::First` 注册该类型的
内部 drain System。同一 `E` 以相同 Options 重复注册为等幂操作，不替换 channel；
使用不同 Options 重复注册时立即给出配置错误。需要不同配置时应使用
不同的 Event newtype。

### 9.4 Sender 与背压

```rust
#[derive(Clone)]
pub struct ExternalEventSender<E: Event>;

impl<E: Event> ExternalEventSender<E> {
    pub fn try_send(&self, event: E) -> Result<(), ExternalEventSendError<E>>;
    pub fn max_capacity(&self) -> usize;
    pub fn is_closed(&self) -> bool;
}

pub enum ExternalEventSendError<E> {
    Full(E),
    Closed(E),
}
```

`try_send()` 永不阻塞，且失败时归还原 Event：

- 入队成功：如果可用，调用一次 `AppControl::wake()`。
- 队列已满：返回 `Full(event)`，不唤醒 App。
- App/receiver 已销毁：返回 `Closed(event)`。

第一版不提供阻塞 `send()` 或 async `send().await`，避免外部生产者因 ECS
消费速度失去控制。

### 9.5 帧语义与顺序

```text
external thread
→ ExternalEventSender<E>::try_send
→ bounded FIFO channel
→ AppControl::wake
→ next App tick / Stage::First
→ drain at most max_events_per_frame
→ World::send_event(E)
→ normal EventReader<E>
```

- 同一 Event 类型内按 channel FIFO 顺序注入 World。
- 不同 Event 类型之间不承诺全局顺序。
- 在该类型 drain System 开始前入队的 Event 可在当前帧进入 World；
  drain 开始后到达的 Event 最迟在下一次 tick 处理。
- 每帧上限防止高频外部输入长时间占用主线程。队列未清空时，
  如果 `AppControl` 可用，drain System 必须再次 `wake()` 以便继续处理。

Event 进入 World 后完全遵循 core Event 的 reader、retention 和过期语义。

### 9.6 HTTP 映射建议

`external_event_plugin` 不定义 HTTP status。未来具有实际业务消费者的 HTTP 适配
Plugin 应统一映射：

```text
try_send Ok          → 202 Accepted + request_id
Full(event)          → 429 Too Many Requests
Closed(event)        → 503 Service Unavailable
invalid request      → 400 Bad Request
authentication fail  → 401/403
```

HTTP handler 生成稳定 `request_id` 并放入业务 Event。结果不通过 channel sender
塞进 ECS Event，而是以携带同一 `request_id` 的业务结果 Event 进入 EventBus，
再由 SSE/WebSocket 或查询 API 交付。

### 9.7 第一版不做

- 外部线程直接访问 World
- 跨 Event 类型全局顺序
- 阻塞或 async sender
- request/response oneshot registry
- 持久化队列和重启恢复
- 自动 retry 或丢弃最旧输入

### 9.8 测试要求

- 有 App runtime 时入队后唤醒 App，并在 `Stage::First` 转为正常 ECS Event。
- 无 App runtime 时手动 tick 仍可正常 drain。
- 验证 FIFO、queue full、closed 和每帧上限。
- 验证队列未清空时会继续 wake。
- 验证相同 Options 重复注册不清空 channel，不同 Options 被拒绝。
- 验证 App Drop 后所有留存 sender 返回 `Closed`。

## 10. 发布级维护规则

### 10.1 稳定性

```text
Stable
  lib.rs 导出的核心类型、Plugin、Options、Event、Handle、Error。

Experimental
  明确标记且可能调整的扩展点。

Internal
  worker、channel、storage、System 函数和 helper。
```

破坏 Stable API 必须提升 SemVer major（1.0 前提升 minor），并在 changelog 中提供迁移说明。

### 10.2 发布前要求

- crate 名称和仓库命名脱离 Margatroid 领域语义
- README、crate-level docs、examples 使用同一套 API
- `cargo doc --no-deps` 无 warning
- `cargo test`、clippy `-D warnings`、fmt check 通过
- `cargo publish --dry-run` 通过
- 明确 MSRV
- 添加 SPDX License、repository、documentation、keywords、categories
- CI 覆盖 Linux、macOS、Windows
- 检查 package 内容，不上传 secret、测试凭据和本地路径
- 使用 `cargo deny` 或同等工具检查 license、advisory 和依赖来源

### 10.3 文档语言

内部验证阶段只维护中文规范。准备发布时：

- crates.io README 和 rustdoc 以英文为主
- 中文设计文档继续保留
- 不在代码 API 中引入本地化字符串
- 错误类型保持结构化，展示层负责 i18n
