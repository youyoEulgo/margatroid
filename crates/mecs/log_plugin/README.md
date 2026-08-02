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
`TracingStream`。
