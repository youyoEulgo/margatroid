# log_plugin

`log_plugin` 为 mecs App 安装开箱即用的进程级 `tracing` subscriber。

```rust
use core_plugin::App;
use log_plugin::LogPlugin;

let mut app = App::new();
app.add_plugins(LogPlugin::default());
tracing::info!("application started");
```

默认向 stderr 输出 `Info` 级 Compact 日志。可选 Layer 包括 console、rolling file
和有界进程内 stream。业务代码继续使用标准 `tracing` 宏，不依赖本 crate。
