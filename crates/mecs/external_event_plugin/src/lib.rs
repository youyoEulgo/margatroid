mod options;
mod plugin;
mod sender;

pub use options::ExternalEventOptions;
pub use plugin::{ExternalEventAppExt, ExternalEventPlugin};
pub use sender::{ExternalEventSendError, ExternalEventSender};
