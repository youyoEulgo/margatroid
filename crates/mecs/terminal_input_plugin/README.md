# TerminalInputPlugin

`TerminalInputPlugin` converts local terminal input into typed ECS events. It supports explicit raw
and cooked modes, terminal resize, focus, paste and mouse events, and restores terminal state during
ordered shutdown or resource drop.

```rust
use core_plugin::App;
use external_event_plugin::ExternalEventPlugin;
use terminal_input_plugin::{TerminalInputOptions, TerminalInputPlugin};

let mut app = App::new();
app.add_plugins(ExternalEventPlugin);
app.add_plugins(TerminalInputPlugin::with_options(
    TerminalInputOptions::raw()
        .with_bracketed_paste(true)
        .with_mouse_capture(true),
));
```

The plugin does not implement `Default`; taking ownership of stdin or enabling raw mode must be an
explicit composition-root decision. It does not render a UI, interpret shortcuts, manage process
signals, or spawn pseudo-terminal child processes. Only one active session may own stdin in a
process; a second session reports `TerminalInputFailureKind::AlreadyInUse`.
