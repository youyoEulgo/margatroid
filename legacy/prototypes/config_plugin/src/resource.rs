use std::fmt;
use std::path::PathBuf;
use std::sync::RwLock;

use types::config::AppConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigStoreError {
    NoConfigLoaded,
}

impl fmt::Display for ConfigStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigStoreError::NoConfigLoaded => write!(f, "no app config has been loaded"),
        }
    }
}

impl std::error::Error for ConfigStoreError {}

#[derive(Clone, Debug)]
pub struct LoadedConfig {
    pub path: PathBuf,
    pub config: AppConfig,
}

pub struct ConfigStore {
    current: RwLock<Option<LoadedConfig>>,
}

impl ConfigStore {
    pub fn new() -> Self {
        Self {
            current: RwLock::new(None),
        }
    }

    pub fn set(&self, path: impl Into<PathBuf>, config: AppConfig) -> Option<LoadedConfig> {
        self.current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(LoadedConfig {
                path: path.into(),
                config,
            })
    }

    pub fn get(&self) -> Option<LoadedConfig> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn config(&self) -> Result<AppConfig, ConfigStoreError> {
        self.get()
            .map(|loaded| loaded.config)
            .ok_or(ConfigStoreError::NoConfigLoaded)
    }
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self::new()
    }
}
