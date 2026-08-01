# sandbox_plugin

`sandbox_plugin` 提供 V3 命令执行边界。

## 职责

- 注册 Sandbox 策略和执行器 Resource。
- 消费 `SandboxCommandRequested`。
- 通过 `AsyncRuntimePlugin` 执行命令。
- 发出命令完成或失败事件。

## 公开事件

- `SandboxCommandRequested`
- `SandboxCommandStarted`
- `SandboxCommandCompleted`
- `SandboxCommandFailed`

## 公开 Resource

- `SandboxPolicy`
- `SandboxExecutor`

## Stage 注册

异步结果在 `Stage::First` 返回；命令结果在 `Stage::Update` 转换为公开事件。

## 最小示例

```rust
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use sandbox_plugin::{SandboxCommandRequested, SandboxPlugin};

let mut app = App::new();
app.add_plugins(AsyncRuntimePlugin::default());
app.add_plugins(SandboxPlugin::new());
app.world().send_event(SandboxCommandRequested::new("cmd-1", "echo ok"));
```

## 边界

该 Plugin 不解释工具语义、不决定 Agent 是否应该运行命令，也不直接读取 LLM 输出。
