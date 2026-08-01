# event_bus_plugin

`event_bus_plugin` 提供 V3 具名事件广播边界。

## 职责

- 注册 `EventBus` Resource。
- 将 `WorkspaceEventEmitted` 事件发布到具名广播 channel。
- 通过 `EventBusPublishFailed` 报告发布失败。
- 为 Server、CLI 或测试代码提供订阅句柄。

## 公开事件

- `WorkspaceEventEmitted`
- `EventBusPublishFailed`

## 公开 Resource

- `EventBus`

## Stage 注册

`EventBusPlugin` 在 `Stage::Update` 注册发布 System。

## 最小示例

```rust
use core_plugin::App;
use event_bus_plugin::{EventBus, EventBusPlugin};

let mut app = App::new();
app.add_plugins(EventBusPlugin::new());

let receiver = app
    .world()
    .resource::<EventBus>()
    .unwrap()
    .register("demo/stream");
```

## 边界

该 Plugin 不定义运行时任务状态、不生成 LLM chunk、不持久化记忆，也不持有 HTTP/SSE
路由。它只提供内部广播 Resource 和事件到 channel 的桥接。
