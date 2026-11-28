# log_plugin

`log_plugin` 为 mecs 应用装配进程级 `tracing` Subscriber。普通诊断通过 `tracing` 宏记录；需要参与 ECS 事件传播的记录使用 `EventLog`，由日志 System 投影到相同的日志输出。

## 安装并记录

```rust
use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;
use log_plugin::{LogLevel, LogPlugin, WorldEventLogExt};

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(LogPlugin::default());
    tracing::info!(component = "example", "application started");
    app.world().event_log(LogLevel::Info, "workspace started");
    app.tick();
}
```

默认配置在 `POST_UPDATE` 处理 `EventLog`，以 Compact 格式向 stderr 输出 Info 及以上级别记录。`in_schedule(name)` 可选择应用已创建的阶段。

## 输出与过滤入口

- `LogPlugin`：通过 `with_level`、`with_filter`、`with_format` 配置过滤与格式。
- `LogLevel` / `LogFormat`：日志级别与 Compact、Pretty、Json 输出格式。
- `ConsoleTarget` / `FileLogOptions`：控制台目标与显式启用的滚动文件输出。
- `EventLog` / `WorldEventLogExt`：创建携带日志级别与消息的 ECS 事件。
- `TracingStream`：通过 `with_stream(capacity)` 启用的有界进程内广播资源。
- `TracingSubscription` / `TracingRecord`：异步订阅读取入口与结构化记录。

`with_filter(expression)` 使用表达式作为完整过滤配置；省略时使用 `with_level` 对应的级别指令。读取环境配置的应用可先取得环境变量，再将表达式传给 `with_filter`。

## 订阅与进程生命周期

启用流后，从 World 取得 `TracingStream` 并调用 `subscribe()`。订阅从创建时开始接收记录，`recv().await` 返回结构化记录；慢订阅者收到 `TracingStreamError::Lagged(count)`，并可通过 `dropped_count()` 查看累计丢失量。流用于实时诊断，持久输出通过 `with_file` 配置。

Subscriber 配置在进程内共享。多个 App 使用相同配置时复用 Subscriber 与流，每个 App 安装自己的日志 System；冲突配置触发 `LogError::ConflictingConfiguration` panic。文件输出的 worker guard 由进程级安装状态持有。

## 继续阅读

- [装配与配置](src/plugin.rs)、[日志选项](src/options.rs)、[订阅流](src/stream.rs)。
- [安装与事件投影测试](tests/install.rs)：并发装配、共享流及同帧事件读取。
- [设计与完整接口](DESIGN.md)。
