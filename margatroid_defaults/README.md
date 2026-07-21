# margatroid_defaults

该 crate 提供 Margatroid V3 的默认 Plugin 组合。

```rust
use core_plugin::App;
use margatroid_defaults::MargatroidDaemonPlugins;

let mut app = App::new();
app.add_plugins(MargatroidDaemonPlugins::default());
```

高级用户仍可以独立安装和配置每个基础设施或业务 Plugin。
