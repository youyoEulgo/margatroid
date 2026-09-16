use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    ReadFailed(PathBuf),
    TooLarge,
    DecodeFailed,
    InvalidServerBind,
    InvalidLogLevel,
    InvalidLogFilter,
    EmptyTargets(&'static str),
    InvalidTarget(&'static str),
    DuplicateTarget(&'static str),
    InvalidIngressAllow,
    InvalidIngressEntry(usize),
    IngressEntryCoversEverything(usize),
    IngressWiderThanBind,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFailed(path) => {
                write!(
                    formatter,
                    "configuration could not be read: {}",
                    path.display()
                )
            }
            Self::TooLarge => formatter.write_str("configuration exceeds the size limit"),
            Self::DecodeFailed => formatter.write_str("configuration could not be decoded"),
            Self::InvalidServerBind => {
                formatter.write_str("configuration field `server.bind` is not a socket address")
            }
            Self::InvalidLogLevel => formatter.write_str(
                "configuration field `log.level` is not off, error, warn, info, debug or trace",
            ),
            Self::InvalidLogFilter => {
                formatter.write_str("configuration field `log.filter` is empty")
            }
            Self::EmptyTargets(field) => {
                write!(formatter, "configuration field `{field}` has no targets")
            }
            Self::InvalidTarget(field) => {
                write!(
                    formatter,
                    "configuration field `{field}` contains an invalid target"
                )
            }
            Self::DuplicateTarget(field) => {
                write!(
                    formatter,
                    "configuration field `{field}` contains a duplicate target"
                )
            }
            Self::InvalidIngressAllow => formatter.write_str(
                "configuration field `server.allow` is not localhost, 127.0.0.1, all, 0.0.0.0 or an array of addresses",
            ),
            Self::InvalidIngressEntry(index) => {
                write!(
                    formatter,
                    "configuration field `server.allow` entry {index} is not an address or CIDR block"
                )
            }
            Self::IngressEntryCoversEverything(index) => {
                write!(
                    formatter,
                    "configuration field `server.allow` entry {index} covers every address"
                )
            }
            Self::IngressWiderThanBind => formatter.write_str(
                "configuration field `server.allow` admits non-loopback peers while `server.bind` is loopback",
            ),
        }
    }
}

impl std::error::Error for ConfigError {}
