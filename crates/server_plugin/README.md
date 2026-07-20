# server_plugin

`server_plugin` provides the V3 daemon HTTP boundary.

## Responsibilities

- Register server config and handle resources.
- Start and stop the daemon HTTP boundary.
- Emit server lifecycle events.

## Public Events

- `ServerStartRequested`
- `ServerStarted`
- `ServerFailed`
- `ShutdownRequested`
- `HttpRequestReceived`
- `UserPromptSubmitted`

## Public Resources

- `ServerConfig`
- `ServerHandle`

## Stage Registration

- Start and shutdown requests are consumed in `Stage::Input`.
- Optional auto-start runs in `Stage::Startup`.

## Minimal Example

```rust
use core_plugin::App;
use server_plugin::{ServerPlugin, ServerStartRequested};

let mut app = App::new();
app.add_plugins(ServerPlugin::new());
app.world().send_event(ServerStartRequested);
app.tick();
```

## Boundaries

This plugin currently owns only the daemon HTTP lifecycle and a `/health` endpoint. It does not perform business scheduling, call LLM providers, execute workflows, or write memory.
