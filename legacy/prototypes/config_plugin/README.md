# config_plugin

`config_plugin` provides the V3 configuration loading boundary.

## Responsibilities

- Register a `ConfigStore` resource.
- Load app configuration from TOML files.
- Emit config load, reload, and failure events.

## Public Events

- `ConfigLoadRequested`
- `ConfigLoaded`
- `ConfigReloaded`
- `ConfigLoadFailed`

## Public Resources

- `ConfigStore`

## Stage Registration

- Optional autoload runs in `Stage::Startup`.
- Explicit load requests are consumed in `Stage::Update`.

## Minimal Example

```rust
use config_plugin::{ConfigLoadRequested, ConfigPlugin};
use core_plugin::App;

let mut app = App::new();
app.add_plugins(ConfigPlugin::new());
app.world().send_event(ConfigLoadRequested::new("config.toml"));
app.tick();
```

## Boundaries

This plugin does not create workspaces, construct LLM providers, or start HTTP servers.
