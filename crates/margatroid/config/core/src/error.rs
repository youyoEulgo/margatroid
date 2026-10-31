use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    ReadFailed(PathBuf),
    TooLarge,
    DecodeFailed,
    InvalidBind,
    InvalidIngress,
    InvalidLogLevel,
    InvalidLogFilter,
    InvalidTarget,
    DuplicateTarget,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFailed(path) => write!(formatter, "cannot read configuration: {}", path.display()),
            Self::TooLarge => formatter.write_str("configuration exceeds the size limit"),
            Self::DecodeFailed => formatter.write_str("configuration is not valid UTF-8 TOML or contains unknown fields"),
            Self::InvalidBind => formatter.write_str("server.bind is not a socket address"),
            Self::InvalidIngress => formatter.write_str("server.allow is invalid or admits non-loopback peers while server.bind is loopback"),
            Self::InvalidLogLevel => formatter.write_str("log.level must be off, error, warn, info, debug or trace"),
            Self::InvalidLogFilter => formatter.write_str("log.filter must not be empty"),
            Self::InvalidTarget => formatter.write_str("outbound groups must be nonempty and contain valid broadcast, type or name targets"),
            Self::DuplicateTarget => formatter.write_str("outbound group contains a duplicate target"),
        }
    }
}

impl std::error::Error for ConfigError {}
