# llm_plugin

`llm_plugin` provides the V3 LLM provider boundary.

## Responsibilities

- Register an `LlmProviderRegistry` resource.
- Consume `LlmRequest` events.
- Call registered `types::DynAiProvider` implementations on the core async worker.
- Emit `LlmResponse`, `LlmStreamChunk`, or `LlmFailed`.

## Public Events

- `LlmRequest`
- `LlmResponse`
- `LlmStreamChunk`
- `LlmFailed`

## Public Resources

- `LlmProviderRegistry`

## Stage Registration

Async results are converted into public LLM events in `Stage::Finalize`.

## Minimal Example

```rust
use core_plugin::App;
use llm_plugin::{LlmPlugin, LlmProviderRegistry};

let mut app = App::new();
app.add_plugins(LlmPlugin::new());

let registry = app.world().resource::<LlmProviderRegistry>().unwrap();
```

## Boundaries

This plugin does not choose which agent should speak, execute workflows, write memory, or send SSE.
