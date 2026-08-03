use std::fmt;

#[non_exhaustive]
#[derive(Debug)]
pub enum ClosureError {
    RuntimePluginMissing,
    ClosurePluginMissing,
    ClosurePluginAlreadyInstalled,
    ClosureSystemAlreadyRegistered { schedule: String },
    ClosureSystemNotRegistered { schedule: String },
}

impl ClosureError {
    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for ClosureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimePluginMissing => formatter.write_str("RuntimePlugin is not installed"),
            Self::ClosurePluginMissing => formatter.write_str("ClosurePlugin is not installed"),
            Self::ClosurePluginAlreadyInstalled => {
                formatter.write_str("ClosurePlugin is already installed")
            }
            Self::ClosureSystemAlreadyRegistered { schedule } => {
                write!(
                    formatter,
                    "a ClosureSystem is already registered for schedule `{schedule}`"
                )
            }
            Self::ClosureSystemNotRegistered { schedule } => {
                write!(
                    formatter,
                    "no ClosureSystem is registered for schedule `{schedule}`"
                )
            }
        }
    }
}

impl std::error::Error for ClosureError {}
