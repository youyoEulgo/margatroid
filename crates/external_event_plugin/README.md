# external_event_plugin

`external_event_plugin` 将外部线程产生的类型化数据通过有界 channel 安全注入
mecs World。

```rust
use app_runtime_plugin::AppRuntimePlugin;
use core_plugin::App;
use external_event_plugin::{ExternalEventAppExt, ExternalEventPlugin};

let mut app = App::new();
app.add_plugins(AppRuntimePlugin);
app.add_plugins(ExternalEventPlugin);
app.add_external_event::<MyEvent>();

let sender = app.external_event_sender::<MyEvent>();
sender.try_send(MyEvent { /* ... */ })?;
```

入队永不阻塞。在 `add_external_event::<E>()` 前安装 `AppRuntimePlugin` 时会自动
唤醒 App；未安装时可由宿主手动调用 `tick()`。
