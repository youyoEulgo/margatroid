use std::path::PathBuf;

use crate::LogError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub(crate) fn directive(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    #[default]
    Compact,
    Pretty,
    Json,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConsoleTarget {
    Stdout,
    #[default]
    Stderr,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogRotation {
    Minutely,
    Hourly,
    #[default]
    Daily,
    Never,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLogOptions {
    pub(crate) directory: PathBuf,
    pub(crate) file_name_prefix: String,
    pub(crate) rotation: LogRotation,
    pub(crate) max_files: Option<usize>,
    pub(crate) non_blocking: bool,
}

impl FileLogOptions {
    pub fn daily<Path, Prefix>(directory: Path, prefix: Prefix) -> Self
    where
        Path: Into<PathBuf>,
        Prefix: Into<String>,
    {
        Self {
            directory: directory.into(),
            file_name_prefix: prefix.into(),
            rotation: LogRotation::Daily,
            max_files: None,
            non_blocking: true,
        }
    }

    pub fn with_rotation(mut self, rotation: LogRotation) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_max_files(mut self, max_files: usize) -> Self {
        if max_files == 0 {
            LogError::InvalidMaxFiles { max_files }.panic();
        }
        self.max_files = Some(max_files);
        self
    }

    pub fn blocking(mut self) -> Self {
        self.non_blocking = false;
        self
    }
}
