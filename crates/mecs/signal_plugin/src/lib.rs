//! Converts configured operating-system signals into typed ECS events.
//!
//! The plugin provides mechanism only: applications decide whether a received signal should stop,
//! reload, pause, or otherwise affect the app.
//!
//! ```no_run
//! use core_plugin::App;
//! use external_event_plugin::ExternalEventPlugin;
//! use signal_plugin::SignalPlugin;
//!
//! let mut app = App::new();
//! app.add_plugins(ExternalEventPlugin);
//! app.add_plugins(SignalPlugin::new());
//! app.tick();
//! ```

mod events;
mod options;
mod plugin;
mod resource;

pub use events::{ProcessSignal, ProcessSignalReceived, SignalListenerFailed};
pub use options::SignalOptions;
pub use plugin::SignalPlugin;
pub use resource::SignalHandle;
