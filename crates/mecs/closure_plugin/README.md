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
app.tick(); // 下一事件帧由目标ClosureSystem执行闭包
```

`send_closure` 内部仍然使用 Runtime 的事件发送入口，因此事件入队与唤醒逻辑只有一份。
`ClosureSystem` 必须由开发者显式挂载；插件不会自行决定哪些 Schedule 可以执行闭包。
请求在下一事件帧可读，闭包按目标 Schedule 中该 System 的注册位置执行。普通事件仍可被多个
System 读取；这里仅通过包装类型中的 `Mutex<Option<_>>` 转移一次闭包所有权。
同步闭包的 panic 会沿当前 tick 传播，插件不捕获它，也不提供取消已排队闭包的 API。
单次 Schedule 仅运行一次，其执行后发送到该阶段的请求不会再被处理。
