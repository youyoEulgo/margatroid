# SignalPlugin

`SignalPlugin` 将配置的操作系统进程信号转换为类型化 ECS 事件。它不决定应用收到信号后
应该关闭、重载、暂停还是忽略。

```rust
use core_plugin::App;
use external_event_plugin::ExternalEventPlugin;
use signal_plugin::{ProcessSignal, SignalOptions, SignalPlugin};

let mut app = App::new();
app.add_plugins(ExternalEventPlugin);
app.add_plugins(SignalPlugin::with_options(
    SignalOptions::new().with_signals([ProcessSignal::Interrupt, ProcessSignal::Terminate]),
));
```

监听器发布 `ProcessSignalReceived`。应用可以消费该事件，并根据自身策略调用
`AppControl::shutdown()`。存在 App Runtime 关闭阶段时，监听线程会在该阶段停止并回收；
对于手动 tick 的应用，则在对应 Resource 被丢弃时停止并回收。
