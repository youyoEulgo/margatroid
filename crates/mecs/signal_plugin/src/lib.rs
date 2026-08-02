//! 将配置的操作系统信号转换为类型化 ECS 事件。
//!
//! Plugin 只提供信号输入机制；应用自行决定收到信号后关闭、重载、暂停或忽略。
//!
//! ```no_run
//! use core_plugin::App;
//! use app_runtime_plugin::RuntimePlugin;
//! use signal_plugin::SignalPlugin;
//!
//! let mut app = App::new();
//! app.add_plugin(RuntimePlugin::default());
//! app.add_plugin(SignalPlugin::new());
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
