use std::path::PathBuf;

use types::config::AppConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigLoadRequested {
    pub path: PathBuf,
}

impl ConfigLoadRequested {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigLoaded {
    pub path: PathBuf,
    pub config: AppConfig,
}

#[derive(Clone, Debug)]
pub struct ConfigReloaded {
    pub path: PathBuf,
    pub config: AppConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigLoadFailed {
    pub path: PathBuf,
    pub message: String,
}
