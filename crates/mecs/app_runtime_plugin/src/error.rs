use std::fmt;

#[non_exhaustive]
#[derive(Debug)]
pub enum RuntimeError {
    InvalidFrameRate { frame_rate: u64 },
    RuntimePluginMissing,
    RuntimePluginAlreadyInstalled,
    RuntimeAlreadyRunning,
    WakeChannelDisconnected,
    GateOperationUnbalanced,
}

impl RuntimeError {
    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameRate { frame_rate } => {
                write!(
                    formatter,
                    "runtime frame rate must be greater than zero, got {frame_rate}"
                )
            }
            Self::RuntimePluginMissing => formatter.write_str("RuntimePlugin is not installed"),
            Self::RuntimePluginAlreadyInstalled => {
                formatter.write_str("RuntimePlugin is already installed")
            }
            Self::RuntimeAlreadyRunning => {
                formatter.write_str("runtime has already started running")
            }
            Self::WakeChannelDisconnected => {
                formatter.write_str("runtime wake channel is disconnected")
            }
            Self::GateOperationUnbalanced => {
                formatter.write_str("runtime gate open and close operations are unbalanced")
            }
        }
    }
}

impl std::error::Error for RuntimeError {}
