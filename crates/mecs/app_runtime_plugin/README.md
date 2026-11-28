# app_runtime_plugin

`app_runtime_plugin` 为 mecs App 提供阻塞式运行循环。默认事件驱动模式根据事件队列推进帧，并在等待期间接收跨线程唤醒；固定帧模式按指定帧率推进应用。

## 发送事件并运行到完成

```rust
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use app_runtime_plugin::{AppRunExt, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Event, World};

struct Notice;
impl Event for Notice {}

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default());
    let done = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&done);
    app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
        for _ in world.event_reader::<Notice>() {
            observed.store(true, Ordering::Release);
        }
    });
    app.world().send_event(Notice);
    app.run_until(|| done.load(Ordering::Acquire));
}
```

`send_event` 将事件入队并唤醒循环。`event_sender()` 返回可克隆的 `RuntimeEventSender`，适合交给外部线程；`send_event_after(event, frames)` 按帧设置延迟。

## 入口与默认阶段

- `RuntimePlugin::default()`：事件驱动循环，可快进至延迟事件的目标帧。
- `RuntimePlugin::fixed(frame_rate)`：固定帧循环。
- `AppRunExt`：`run()` 持续运行，`run_until(predicate)` 在谓词成立时返回。
- `WorldEventExt`：取得发送器，或发送普通、延迟事件。
- `RuntimeEventSender`：跨线程发送事件并唤醒所属 World 的运行循环。
- `RuntimeHandle`：调用 `wake()` 唤醒，或通过 `close_gate()` / `open_gate()` 管理帧推进阀。

安装时按顺序创建 `STARTUP`、`PRE_UPDATE`、`UPDATE`、`POST_UPDATE`。`STARTUP` 在首次 tick 执行一次，其余阶段每帧执行。阶段内按 System 注册顺序执行，事件按 Core 的帧边界分发。

## 停止与句柄生命周期

`run_until` 在初始帧前、循环前及唤醒后检查谓词。外部线程请求停止时，先写入共享状态，再调用 `RuntimeHandle::wake()`，让事件驱动等待及时重新检查。固定帧模式的停止延迟受当前帧间隔影响。

运行阀使用阻塞计数，所有关闭操作各自配对一次开启操作。停止谓词在关阀期间同样参与检查。循环返回后会恢复 World 中的运行控制资源，应用可以继续 `tick()` 或再次调用 `run_until`。

业务任务的停止与清理由应用组织。发送器和唤醒句柄绑定创建它们的 World；在销毁对应 Runtime 前，先停止生产者并释放外部克隆。

## 继续阅读

- [扩展 trait 与循环实现](src/plugin.rs)、[跨线程句柄](src/resource.rs)。
- [运行到停止条件的测试](tests/run_until.rs)：外部唤醒、关阀停止、延迟事件及再次运行。
- [设计与接口](DESIGN.md)。
