# mecs 公开 API

状态：V3 目标契约

本文是 mecs 的唯一公开 API 设计文档。mecs 是可独立发布的同步 ECS 与基础设施
Plugin 集合，不包含 Agent、Workspace、LLM 等 Margatroid 业务概念。所有新增 API 先按
[V3-DESIGN.md 的 API 设计方法论](V3-DESIGN.md#9-api-设计方法论)审查。

## 1. 设计规则

- core 只保留 Entity、Component、Resource、System、Schedule、Event 和 Plugin 成立所需能力。
- 所有 System 都是同步函数，只有主线程可以修改 World。
- 一项能力只有一个写入口。
- 公开类型必须是命令、结果、只读状态、控制句柄或扩展点之一。
- 默认配置必须可用；高级配置集中在一个 Options 中。
- Plugin 按依赖顺序安装，关闭回调按安装的逆序执行。
- 可选能力使用独立 Plugin，不使用 core feature 隐式开启。
- 未实现的能力不进入稳定 API。

## 2. Crate 边界

```text
core_plugin             同步 ECS 与单帧执行
app_runtime_plugin      阻塞运行循环、唤醒和关闭
external_event_plugin   外部线程向 ECS 发送有界 Event
async_runtime_plugin    Future 执行与结果回流
signal_plugin           进程信号转 Event
terminal_input_plugin   终端输入转 Event
log_plugin              tracing 配置、SystemLog 与 EventLog 投影
server_plugin           HTTP、WebSocket、流式通道与服务生命周期
```

core 不依赖任何其他 Plugin，也不依赖 Tokio。基础设施 Plugin 可以依赖已稳定的下层
Plugin，例如 async_runtime_plugin 和 log_plugin 依赖 app_runtime_plugin 的默认
Schedule。

## 3. core_plugin

### 3.1 稳定导出

```rust
pub use app::{App, Stage};
pub use component::{Bundle, Component};
pub use entity::Entity;
pub use events::{Event, EventReader};
pub use plugin::{Plugin, PluginGroup};
pub use query::{Query, QueryMut};
pub use resource::Resource;
pub use system::{named_system, System, SystemFailed, WrappedFn};
pub use world::World;
```

`Schedule`、执行报告、事件存储和系统排序实现不公开。`Res`、`ResMut` 在自动 System
参数存在前不公开。

### 3.2 App

```rust
impl App {
    pub fn new() -> Self;
    pub fn add_plugins(&mut self, plugins: impl Plugin) -> &mut Self;
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
    pub fn set_event_retention_frames(&mut self, frames: u64) -> &mut Self;
    pub fn tick(&mut self);
}
```

`App` 不提供 `run`、异步、网络和日志快捷方法。`Stage` 固定为：

```text
Startup（一次） -> First -> Update -> Last -> Event maintenance
```

### 3.3 World

```text
Entity:   spawn / despawn / is_alive / entity_count
Component: insert / remove / get / get_mut / has / iter / iter_mut
Resource: add_resource / remove_resource / resource / resource_mut
Event:    emit_event / event_reader
```

### 3.4 System 与顺序

普通闭包 `FnMut(&mut World) + Send + 'static` 自动成为 System。无约束 System 保持注册
顺序；需要排序时使用 `named_system(...).before(...).after(...)`。配置错误和 panic 统一产生
`SystemFailed`，不公开 Schedule 报告对象。

### 3.5 Event

每种 Event 有独立队列，每个 `EventReader<E>` 有独立游标。Reader 默认只读取创建后发送的
事件。Event 按帧保留，读取返回值，不暴露存储锁。

### 3.6 Plugin

```rust
pub trait Plugin {
    fn build(&self, app: &mut App);
}
```

PluginGroup 本身也实现 Plugin。`build` 只做组合和注册；启动失败通过 Startup Event
报告。构造函数只保存配置，不执行 I/O。

## 4. app_runtime_plugin

```rust
pub use plugin::{AppRunExt, AppRuntimePlugin, AppShutdownExt};
pub use resource::AppControl;

pub trait AppRunExt {
    fn run(&mut self);
}

pub trait AppShutdownExt {
    fn on_shutdown(
        &mut self,
        system: impl FnMut(&mut World) + Send + 'static,
    ) -> &mut Self;
    fn after_shutdown(
        &mut self,
        system: impl FnMut(&mut World) + Send + 'static,
    ) -> &mut Self;
}
```

`AppControl` 只提供 `wake`、`shutdown` 和 `is_shutdown`。关闭回调按注册逆序执行：依赖先
安装，依赖者后安装，因此依赖者先关闭。`after_shutdown` 只用于所有依赖关闭后的最终状态；
不公开固定的 Begin/Workers/Finish 阶段。

## 5. external_event_plugin

```rust
pub use options::ExternalEventOptions;
pub use plugin::{ExternalEventAppExt, ExternalEventPlugin};
pub use sender::{ExternalEventSendError, ExternalEventSender};
```

`App::external_event_sender::<E>()` 是外部线程发送 E 的唯一入口。sender 有界、非阻塞，
发送成功时唤醒 App；First 阶段把外部事件写入 World。

## 6. async_runtime_plugin

```rust
pub use error::{AsyncRuntimeError, AsyncTaskError};
pub use plugin::{AppAsyncExt, AsyncRuntimePlugin};
pub use request::{AsyncRequest, AsyncRequestMode, AsyncTask, WorldAsyncExt};
pub use context::AsyncContext;
pub use resource::AsyncRuntimeHandle;
```

`AppAsyncExt::add_async_system::<T, E>(schedule)` 注册 `AsyncRequest<T, E>` 与
`Result<T, E>` 事件，并将通用异步分发 System 挂到开发者指定的
Schedule。`WorldAsyncExt::send_async_event` 将异步闭包封装为请求事件。业务
Plugin 自行挂载最终的 `Result<T, E>` 响应处理 System。异步函数可以声明
`AsyncContext` 参数，由 AsyncRuntime 自动注入并用于在任务完成前发送普通事件。
`WorldAsyncExt::spawn_async_service` 用于 Server 等长期基础设施 Future，不创建 pending
事件、不操作 Runtime gate，也不自动产生完成事件。

## 7. signal_plugin

```rust
pub use events::{ProcessSignal, ProcessSignalReceived, SignalListenerFailed};
pub use options::SignalOptions;
pub use plugin::SignalPlugin;
pub use resource::SignalHandle;
```

SignalPlugin 只把信号转换为 Event，不决定关闭、重载或暂停。Handle 仅用于查询和提前停止
监听器。

## 8. terminal_input_plugin

```rust
pub use events::{TerminalEvent, TerminalInputFailed, TerminalInputFailureKind, TerminalSize};
pub use options::TerminalInputOptions;
pub use plugin::TerminalInputPlugin;
pub use resource::{TerminalError, TerminalSessionHandle};
```

Plugin 负责终端会话的 RAII 恢复。输入通过 `TerminalEvent` 统一输出；无需独立消费的内部
失败分类不形成额外 Event。Key 和 Mouse 数据直接使用并重导出 crossterm 的公开类型，避免
再维护一套字段相同的包装结构。

## 9. log_plugin

直接调用 `tracing` 宏的进程诊断称为 SystemLog。`LogPlugin` 安装 console、rolling
file 和可选 bounded stream Layer，默认只启用 stderr Console Layer。

`EventLog` 通过 ECS 事件队列传播，由 `LogPlugin` 注册在 `RuntimePlugin::POST_UPDATE` 的 System
投影为 `target = "mecs::event_log"` 的 tracing Event。其他 Plugin 可以独立读取同一帧
`EventLog`，不会与日志 System 互相消费。

稳定公开面为 `LogPlugin`、`LogError`、`LogLevel`、`LogFormat`、
`ConsoleTarget`、`LogRotation`、`FileLogOptions`、`EventLog`、
`WorldEventLogExt`、`TracingRecord`、`TracingField`、`TracingStream`、
`TracingSubscription` 和 `TracingStreamError`。tracing Subscriber 的组合类型不公开。

## 10. server_plugin

```rust
pub use events::{HttpRequestReceived, ServerFailed, ServerStarted, ServerStopped};
pub use options::ServerOptions;
pub use plugin::{AppServerExt, ServerPlugin};
pub use resource::ServerHandle;
pub use response::{HttpResponse, HttpResponseHead, HttpResponseSession};
pub use websocket::{
    JsonWebSocketMessageClassifier, WebSocketConnected, WebSocketConnectionId,
    WebSocketConnections, WebSocketDisconnected, WebSocketMessageClassifier,
    WebSocketMessageReceived, WebSocketNameError, WebSocketSender,
    WebSocketStreamOpened, WebSocketStreamReceiver,
};
```

`AppServerExt` 支持原生 Axum Router、HTTP 事件委托和 WebSocket 事件路由。HTTP 流式响应
通过可转交的 `HttpResponseSession` 发送；WebSocket 普通消息进入 ECS，带
`Start/Chunk/End/Abort` 信封的连续分片进入有界支流通道。Handle 提供
`is_running`、`local_address` 和 `shutdown`。

`WebSocketConnections` 由 ServerPlugin 自动维护并与 Axum 共享。连接建立时先注册不具名
发送器，再发送只携带连接 ID 的连接通知；连接断开时先删除注册表条目及名称索引，再发送
断开通知。发送器可以按连接 ID、唯一非空名称或不具名集合查询。业务 endpoint 和消息
语义属于业务 Plugin。

## 11. 稳定性

- `lib.rs` 未导出的类型都不是兼容承诺。
- Options 新增字段必须有默认值。
- Error 和 Event 不包含 token、Authorization header、完整隐私正文或无限制输出。
- Pty、并行 Schedule、自动 System 参数等能力先进入路线图，确认调用场景后再设计 API。
