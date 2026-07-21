use std::path::{Path, PathBuf};

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsoleOptions {
    target: ConsoleTarget,
}

impl ConsoleOptions {
    pub fn stdout() -> Self {
        Self {
            target: ConsoleTarget::Stdout,
        }
    }

    pub fn stderr() -> Self {
        Self::default()
    }

    pub fn target(&self) -> ConsoleTarget {
        self.target
    }
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
    directory: PathBuf,
    file_name_prefix: String,
    rotation: LogRotation,
    max_files: Option<usize>,
    non_blocking: bool,
}

impl FileLogOptions {
    pub fn daily(directory: impl Into<PathBuf>, prefix: impl Into<String>) -> Self {
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
        assert!(max_files > 0, "max_files must be greater than zero");
        self.max_files = Some(max_files);
        self
    }

    pub fn blocking(mut self) -> Self {
        self.non_blocking = false;
        self
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn file_name_prefix(&self) -> &str {
        &self.file_name_prefix
    }

    pub fn rotation(&self) -> LogRotation {
        self.rotation
    }

    pub fn max_files(&self) -> Option<usize> {
        self.max_files
    }

    pub fn is_non_blocking(&self) -> bool {
        self.non_blocking
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogStreamOptions {
    capacity: usize,
}

impl LogStreamOptions {
    pub const DEFAULT_CAPACITY: usize = 1024;

    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "log stream capacity must be greater than zero"
        );
        Self { capacity }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for LogStreamOptions {
    fn default() -> Self {
        Self {
            capacity: Self::DEFAULT_CAPACITY,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogOptions {
    pub(crate) level: LogLevel,
    pub(crate) filter: Option<String>,
    pub(crate) format: LogFormat,
    pub(crate) console: Option<ConsoleOptions>,
    pub(crate) file: Option<FileLogOptions>,
    pub(crate) stream: Option<LogStreamOptions>,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            filter: None,
            format: LogFormat::Compact,
            console: Some(ConsoleOptions::stderr()),
            file: None,
            stream: None,
        }
    }
}

impl LogOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }

    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    pub fn with_format(mut self, format: LogFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_console(mut self, options: ConsoleOptions) -> Self {
        self.console = Some(options);
        self
    }

    pub fn without_console(mut self) -> Self {
        self.console = None;
        self
    }

    pub fn with_file(mut self, options: FileLogOptions) -> Self {
        self.file = Some(options);
        self
    }

    pub fn with_stream(mut self, options: LogStreamOptions) -> Self {
        self.stream = Some(options);
        self
    }
}
