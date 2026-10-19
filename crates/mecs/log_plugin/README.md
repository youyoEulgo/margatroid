# log_plugin

## 介绍

`log_plugin` 为 mecs App 安装进程级 `tracing` Subscriber，并将通过 ECS
传播的 `EventLog` 投影为 tracing Event。

日志遵循 Rust `tracing` 的正常使用方式，不为了架构一致性牺牲诊断路径性能。Plugin 只封装
Subscriber 和 Layer 配置；需要 ECS 传播语义的日志才使用 EventLog。

SystemLog 直接通过 tracing 宏记录，由 Layer 决定去向。EventLog 先通过 ECS 传播，再由日志
System 汇入 tracing。每个进程只安装一个 Subscriber，但可以组合多个 Layer；Plugin 配置
日志机制，业务仍然决定记录什么。

## 使用说明

```rust
use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;
use log_plugin::{LogLevel, LogPlugin, WorldEventLogExt};

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(LogPlugin::default());

tracing::info!("system diagnostic");
app.world().event_log(LogLevel::Info, "workspace started");
app.tick();
```

默认向 stderr 输出 `Info` 级 Compact 日志。可选 Layer 包括滚动文件和有界进程内
`TracingStream`；默认不启用这两者。`with_filter` 的显式表达式完全覆盖
`with_level` 生成的过滤指令，不与默认级别叠加，也不自动读取 `RUST_LOG`。

`TracingStream` 是临时的有界广播：订阅只接收订阅后的记录，无接收者时直接丢弃，
慢订阅者收到 `Lagged` 并累计丢失数。它不保存历史、不重放、不保证可靠送达或持久化；
文件输出必须显式配置。应用守护进程采用的 stderr 加容量 256 诊断流是应用配置，
不是 `LogPlugin::default()` 的框架默认值。
