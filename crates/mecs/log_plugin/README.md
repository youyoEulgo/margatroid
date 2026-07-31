# log_plugin

`log_plugin` 为 mecs App 安装进程级 `tracing` Subscriber，并将通过 ECS
传播的 `EventLog` 投影为 tracing Event。

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
`TracingStream`。直接调用 tracing 宏的日志称为 `SystemLog`；通过 ECS 事件队列传播、
再由日志 System 汇入 tracing 的日志称为 `EventLog`。
