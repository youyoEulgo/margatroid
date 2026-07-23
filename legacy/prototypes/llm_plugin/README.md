# llm_plugin

`llm_plugin` provides the V3 LLM provider boundary.

## Responsibilities

- Register an `LlmProviderRegistry` resource.
- Consume `LlmRequest` events.
- Call registered `types::DynAiProvider` implementations through `AsyncRuntimePlugin`.
- Emit `LlmResponse`, `LlmStreamChunk`, or `LlmFailed`.

## Public Events

- `LlmRequest`
- `LlmResponse`
- `LlmStreamChunk`
- `LlmFailed`

## Public Resources

- `LlmProviderRegistry`

## Stage Registration

Async completions return in `Stage::First`; LLM batch results are converted into public events in `Stage::Update`.

## Minimal Example

```rust
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use llm_plugin::{LlmPlugin, LlmProviderRegistry};

let mut app = App::new();
app.add_plugins(AsyncRuntimePlugin::default());
app.add_plugins(LlmPlugin::new());

let registry = app.world().resource::<LlmProviderRegistry>().unwrap();
```

## Boundaries

This plugin does not choose which agent should speak, execute workflows, write memory, or send SSE.
