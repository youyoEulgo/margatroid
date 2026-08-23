use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    ReadFailed(PathBuf),
    TooLarge,
    DecodeFailed,
    InvalidServerBind,
    EmptyTargets(&'static str),
    InvalidTarget(&'static str),
    DuplicateTarget(&'static str),
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
        }
    }
}

impl std::error::Error for ConfigError {}
