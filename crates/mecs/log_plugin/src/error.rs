use std::error::Error;
use std::fmt;
use std::path::PathBuf;

type BoxedError = Box<dyn Error + Send + Sync>;

#[non_exhaustive]
#[derive(Debug)]
pub enum LogError {
    InvalidFilter {
        filter: String,
        source: BoxedError,
    },
    InvalidMaxFiles {
        max_files: usize,
    },
    InvalidStreamCapacity {
        capacity: usize,
    },
    FileOutputInitFailed {
        directory: PathBuf,
        source: BoxedError,
    },
    SubscriberAlreadyInstalled,
    ConflictingConfiguration,
    LogPluginAlreadyInstalled,
}

impl LogError {
    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for LogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFilter { filter, .. } => {
                write!(formatter, "invalid tracing filter `{filter}`")
            }
            Self::InvalidMaxFiles { max_files } => {
                write!(
                    formatter,
                    "maximum log file count must be greater than zero, got {max_files}"
                )
            }
            Self::InvalidStreamCapacity { capacity } => {
                write!(
                    formatter,
                    "tracing stream capacity must be greater than zero, got {capacity}"
                )
            }
            Self::FileOutputInitFailed { directory, .. } => {
                write!(
                    formatter,
                    "failed to initialize log output in `{}`",
                    directory.display()
                )
            }
            Self::SubscriberAlreadyInstalled => {
                formatter.write_str("a global tracing subscriber is already installed")
            }
            Self::ConflictingConfiguration => formatter.write_str(
                "the requested tracing configuration conflicts with the installed configuration",
            ),
            Self::LogPluginAlreadyInstalled => {
                formatter.write_str("LogPlugin is already installed in this App")
            }
        }
    }
}

impl Error for LogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidFilter { source, .. } | Self::FileOutputInitFailed { source, .. } => {
                Some(source.as_ref())
            }
            _ => None,
        }
    }
}
