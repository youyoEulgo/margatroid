# llm_plugin

`llm_plugin` 提供 V3 LLM Provider 边界。

## 职责

- 注册 `LlmProviderRegistry` Resource。
- 消费 `LlmRequest` 事件。
- 通过 `AsyncRuntimePlugin` 调用已注册的 `types::DynAiProvider` 实现。
- 发出 `LlmResponse`、`LlmStreamChunk` 或 `LlmFailed`。

## 公开事件

- `LlmRequest`
- `LlmResponse`
- `LlmStreamChunk`
- `LlmFailed`

## 公开 Resource

- `LlmProviderRegistry`

## Stage 注册

异步结果在 `Stage::First` 返回；LLM 批量结果在 `Stage::Update` 转换为公开事件。

## 最小示例

```rust
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use llm_plugin::{LlmPlugin, LlmProviderRegistry};

let mut app = App::new();
app.add_plugins(AsyncRuntimePlugin::default());
app.add_plugins(LlmPlugin::new());

let registry = app.world().resource::<LlmProviderRegistry>().unwrap();
```

## 边界

该 Plugin 不选择由哪个 Agent 发言、不执行 Workflow、不写入记忆，也不发送 SSE。
