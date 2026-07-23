//! Converts local terminal input into typed ECS events and restores terminal state with RAII.
//!
//! Raw and cooked modes are explicit because acquiring stdin and changing terminal modes are
//! process-level side effects.
//!
//! ```no_run
//! use core_plugin::App;
//! use external_event_plugin::ExternalEventPlugin;
//! use terminal_input_plugin::{TerminalInputOptions, TerminalInputPlugin};
//!
//! let mut app = App::new();
//! app.add_plugins(ExternalEventPlugin);
//! app.add_plugins(TerminalInputPlugin::with_options(TerminalInputOptions::raw()));
//! app.tick();
//! ```

mod events;
mod options;
mod plugin;
mod resource;

pub use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind as KeyState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
pub use events::{TerminalEvent, TerminalInputFailed, TerminalInputFailureKind, TerminalSize};
pub use options::TerminalInputOptions;
pub use plugin::TerminalInputPlugin;
pub use resource::{TerminalError, TerminalSessionHandle};
