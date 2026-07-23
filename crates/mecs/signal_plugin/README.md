# SignalPlugin

`SignalPlugin` converts configured operating-system process signals into typed ECS events. It does
not decide whether an application should shut down, reload, pause, or ignore a signal.

```rust
use core_plugin::App;
use external_event_plugin::ExternalEventPlugin;
use signal_plugin::{ProcessSignal, SignalOptions, SignalPlugin};

let mut app = App::new();
app.add_plugins(ExternalEventPlugin);
app.add_plugins(SignalPlugin::with_options(
    SignalOptions::new().with_signals([ProcessSignal::Interrupt, ProcessSignal::Terminate]),
));
```

The listener publishes `ProcessSignalReceived`. Applications may consume that event and call
`AppControl::shutdown()` as an explicit policy. The listener thread is closed and joined through
the app runtime shutdown phases when available, or when its resource is dropped in manual-tick
applications.
