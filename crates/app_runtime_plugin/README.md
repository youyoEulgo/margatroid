# AppRuntimePlugin

`AppRuntimePlugin` adds the blocking application loop without expanding the ECS core.

## Public API

- `AppRuntimePlugin`
- `AppRunExt`
- `AppControl`

## Responsibilities

- Repeatedly call `App::tick()`.
- Block while no wake request is pending.
- Expose thread-safe wake and shutdown control.

It does not define business stages, own ECS data, or execute async tasks.

```rust
use app_runtime_plugin::{AppRunExt, AppRuntimePlugin};
use core_plugin::App;

let mut app = App::new();
app.add_plugins(AppRuntimePlugin);
app.run();
```
