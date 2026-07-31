# RuntimePlugin

`RuntimePlugin` provides the blocking application loop without expanding the ECS core.

Installing it also registers four schedules in execution order:

- `STARTUP` runs once on the first tick.
- `PRE_UPDATE`, `UPDATE`, and `POST_UPDATE` run on every tick.

## Public API

- `RuntimePlugin::default()` for event-driven execution
- `RuntimePlugin::fixed(frame_rate)` for fixed-frame execution
- `AppRunExt::run`
- `RuntimeHandle::wake` / `open_gate` / `close_gate`
- `WorldEventExt::emit_event` / `emit_event_after`
- `RuntimeMode` and `RuntimeState`

## Responsibilities

- Drive `App::tick()` in fixed-frame mode.
- Fast-forward delayed events and drive `App::fast_forward_tick()` in event-driven mode.
- Wait while only pending events remain or the blocker count is nonzero.
- Expose a cloneable cross-thread wake and blocker handle.
- Wake the runtime when events are emitted through the runtime extension API.

It does not execute asynchronous tasks or complete pending events.

```rust
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use core_plugin::App;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default()).run();
```
