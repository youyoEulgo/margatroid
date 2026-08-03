# ClosurePlugin

`ClosurePlugin` 允许把一次性同步闭包发送到指定 Schedule。闭包执行时临时取得
`&mut World`，执行结束后立即释放借用。

```rust
use app_runtime_plugin::RuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin, WorldClosureExt};
use core_plugin::App;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::UPDATE);

app.world().send_closure(RuntimePlugin::UPDATE, |world| {
    // 在UPDATE阶段同步访问world
});
```

`send_closure` 内部仍然使用 Runtime 的事件发送入口，因此事件入队与唤醒逻辑只有一份。
`ClosureSystem` 必须由开发者显式挂载；插件不会自行决定哪些 Schedule 可以执行闭包。
