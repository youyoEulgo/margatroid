# AsyncRuntimePlugin

`AsyncRuntimePlugin` adds asynchronous task execution without placing Tokio in `core_plugin`.

## Public API

- `AsyncRuntimePlugin`
- `AsyncRuntimeOptions`
- `AsyncAppExt`
- `AsyncTasks`
- `AsyncSystemOptions`
- `AsyncTaskId`
- `AsyncRuntimeStatus`
- `AsyncTaskStarted`, `AsyncTaskFailed`, `AsyncTaskFailureKind`

## Stage Registration

- `Stage::Startup`: start the managed worker.
- `Stage::First`: apply completions to the main-thread `World`.
- `Stage::Last`: dispatch request events registered through `AsyncAppExt`.

Each request is spawned independently. Futures never borrow `World`; successful outputs return as typed events on a later frame.
The managed worker aborts remaining tasks and joins during reverse-order app shutdown.

```rust
use async_runtime_plugin::{AsyncAppExt, AsyncRuntimePlugin};
use core_plugin::App;

let mut app = App::new();
app.add_plugins(AsyncRuntimePlugin::default());
app.add_async_system(|request: MyRequest| async move {
    MyOutput::from(request)
});
```
