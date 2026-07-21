# event_bus_plugin

`event_bus_plugin` provides the V3 named event broadcast boundary.

## Responsibilities

- Register an `EventBus` resource.
- Publish `WorkspaceEventEmitted` events to named broadcast channels.
- Report publish failures through `EventBusPublishFailed`.
- Provide subscription handles for server, CLI, or test code.

## Public Events

- `WorkspaceEventEmitted`
- `EventBusPublishFailed`

## Public Resources

- `EventBus`

## Stage Registration

`EventBusPlugin` registers its publishing system in `Stage::Update`.

## Minimal Example

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

## Boundaries

This plugin does not define runtime task state, generate LLM chunks, persist memory, or own HTTP/SSE routes. It only provides the internal broadcast resource and the event-to-channel bridge.
