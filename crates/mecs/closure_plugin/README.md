# closure_plugin

`closure_plugin` 将一次性同步闭包投递到指定 Schedule。闭包在目标阶段取得 `&mut World`，适合将外部线程的状态更新交回 ECS 线程。

## 注册阶段并发送闭包

```rust
use app_runtime_plugin::RuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin, WorldClosureExt};
use core_plugin::{App, Resource};

struct Counter(u32);
impl Resource for Counter {}

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE);
    app.world_mut().insert_resource(Counter(0));
    app.world().send_closure(RuntimePlugin::UPDATE, |world| {
        world.get_resource_mut::<Counter>().unwrap().0 += 1;
    });
    app.tick();
    assert_eq!(app.world().get_resource::<Counter>().unwrap().0, 1);
}
```

## 公开入口与执行时机

- `ClosurePlugin`：安装闭包阶段注册表，依赖先安装的 `RuntimePlugin`。
- `AppClosureExt::add_closure_system`：在目标 Schedule 的当前位置注册执行器。
- `WorldClosureExt`：通过 `send_closure` 发送，或用 `closure_sender` 取得绑定阶段的发送器。
- `WorldClosureSender`：可克隆并移入其他线程，通过 `send` 提交闭包。
- `ClosureError`：安装与阶段注册错误。

闭包满足 `FnOnce(&mut World) + Send + 'static`。请求经 Runtime 事件入口入队并唤醒，在后续事件帧由目标执行器取出一次。闭包按执行器在 Schedule 中的注册位置运行，执行结束释放 World 借用；panic 沿当前 tick 传播。

为持续接收投递选择循环 Schedule。单次 Schedule 的接收窗口是它实际执行的那一帧。发送器绑定当前 World 与已注册阶段，在 Runtime 销毁前停止外部生产者并释放克隆。

[发送器接入测试](tests/sender.rs) 展示跨线程投递；[执行器实现与测试](src/plugin.rs) 展示阶段选择和执行顺序；完整接口见 [DESIGN.md](DESIGN.md)。
