# RuntimePlugin

## 介绍

`RuntimePlugin` 在不扩大 ECS Core 职责的前提下提供阻塞式应用运行循环。

Runtime 只决定何时推进 App，不接管事件、异步任务或业务生命周期。唤醒通知只是“重新检查”
的提示，可以合并；真正决定是否继续工作的始终是事件快照和阀状态。

安装 Runtime 后应优先使用 `send_event`，让事件入队后自动唤醒运行循环。固定帧模式按时间
等待，事件驱动模式可以快进延迟事件；关阀会阻止下一帧开始，重新开阀后继续。

## 使用说明

```rust
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use core_plugin::App;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default()).run();
```

安装后会按照执行顺序注册四个 Schedule：

- `RuntimePlugin::STARTUP` 在第一次 tick 时执行一次。
- `RuntimePlugin::PRE_UPDATE`、`RuntimePlugin::UPDATE` 和
  `RuntimePlugin::POST_UPDATE` 在每次 tick 时执行。

## 公开 API

- `RuntimePlugin::default()`：事件驱动模式
- `RuntimePlugin::fixed(frame_rate)`：固定帧模式
- `RuntimePlugin::STARTUP` / `PRE_UPDATE` / `UPDATE` / `POST_UPDATE`：默认 Schedule
- `AppRunExt::run`
- `RuntimeHandle::wake` / `open_gate` / `close_gate`
- `RuntimeEventSender::send_event` / `send_event_after`
- `WorldEventExt::event_sender` / `send_event` / `send_event_after`
- `RuntimeMode` 与 `RuntimeState`

## 职责

- 在固定帧模式下驱动 `App::tick()`。
- 在事件驱动模式下快进延迟事件并驱动 `App::fast_forward_tick()`。
- 只剩 pending 事件或阻塞计数非零时进入等待。
- 提供可克隆、可跨线程使用的唤醒与阻塞句柄。
- 通过 Runtime 拓展 API 发送事件时唤醒运行循环。

它不执行异步任务，也不负责完成 pending 事件。

Core 的 `World::emit_event` 只将事件写入队列；Runtime 提供的
`WorldEventExt::send_event` 和 `send_event_after` 会在写入后额外唤醒运行循环。
`event_sender()` 返回的 `RuntimeEventSender` 可以克隆并交给其他线程，行为相同。
安装 Runtime 后应优先使用 `send_event`；`emit_event` 只保留为不依赖 Runtime 的底层能力。
事件类型由 Core 在首次到期时自动建立读取存储，不需要预先注册。
