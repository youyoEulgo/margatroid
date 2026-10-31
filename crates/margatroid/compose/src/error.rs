use std::fmt;
use std::path::PathBuf;
#[derive(Debug)]
pub struct ComposeError {
    pub domain: &'static str,
    pub path: PathBuf,
    pub message: String,
}
impl ComposeError {
    pub(crate) fn new(
        domain: &'static str,
        path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            domain,
            path: path.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}: {}",
            self.domain,
            self.path.display(),
            self.message
        )
    }
}
impl std::error::Error for ComposeError {}
