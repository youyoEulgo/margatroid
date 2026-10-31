use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentDriverError {
    InvalidPath,
    FileMissing,
    TooLarge,
    InvalidUtf8,
    InvalidManifest,
    InvalidDefaultSettings,
    ReadFailed(String),
}

impl fmt::Display for AgentDriverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath => formatter.write_str("invalid driver file path"),
            Self::FileMissing => formatter.write_str("required driver file is missing"),
            Self::TooLarge => formatter.write_str("driver file exceeds its byte limit"),
            Self::InvalidUtf8 => formatter.write_str("driver file is not valid UTF-8"),
            Self::InvalidManifest => formatter.write_str("invalid driver manifest"),
            Self::InvalidDefaultSettings => formatter.write_str("invalid default driver settings"),
            Self::ReadFailed(message) => write!(formatter, "cannot read driver file: {message}"),
        }
    }
}

impl std::error::Error for AgentDriverError {}
