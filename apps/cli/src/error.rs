use std::fmt;

#[derive(Debug)]
pub enum CliError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Transport(tokio_tungstenite::tungstenite::Error),
    Remote(String),
    Timeout(&'static str),
    Disconnected(&'static str),
    Interrupted(&'static str),
}
impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Transport(error) => write!(f, "{error}"),
            Self::Remote(message) => f.write_str(message),
            Self::Timeout(message) | Self::Disconnected(message) | Self::Interrupted(message) => {
                f.write_str(message)
            }
        }
    }
}
impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Transport(error) => Some(error),
            _ => None,
        }
    }
}
impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
impl From<tokio_tungstenite::tungstenite::Error> for CliError {
    fn from(error: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Transport(error)
    }
}
