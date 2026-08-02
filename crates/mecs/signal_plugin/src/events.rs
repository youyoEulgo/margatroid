use core_plugin::Event;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProcessSignal {
    Interrupt,
    Terminate,
    Hangup,
    Quit,
    WindowChanged,
    User1,
    User2,
    #[cfg(unix)]
    Raw(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessSignalReceived {
    pub signal: ProcessSignal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalListenerFailed {
    pub message: String,
}

impl Event for ProcessSignalReceived {}
impl Event for SignalListenerFailed {}
