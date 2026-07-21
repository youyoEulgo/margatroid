# sandbox_plugin

`sandbox_plugin` provides the V3 command execution boundary.

## Responsibilities

- Register sandbox policy and executor resources.
- Consume `SandboxCommandRequested`.
- Execute commands through `AsyncRuntimePlugin`.
- Emit command completion or failure events.

## Public Events

- `SandboxCommandRequested`
- `SandboxCommandStarted`
- `SandboxCommandCompleted`
- `SandboxCommandFailed`

## Public Resources

- `SandboxPolicy`
- `SandboxExecutor`

## Stage Registration

Async completions return in `Stage::First`; command results are converted into public events in `Stage::Update`.

## Minimal Example

```rust
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use sandbox_plugin::{SandboxCommandRequested, SandboxPlugin};

let mut app = App::new();
app.add_plugins(AsyncRuntimePlugin::default());
app.add_plugins(SandboxPlugin::new());
app.world().send_event(SandboxCommandRequested::new("cmd-1", "echo ok"));
```

## Boundaries

This plugin does not interpret tool semantics, decide whether an agent should run a command, or read LLM output directly.
