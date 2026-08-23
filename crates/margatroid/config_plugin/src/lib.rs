mod error;
mod events;
mod handler;
mod system;
mod types;

use std::fs;
use std::path::PathBuf;

use core_plugin::{App, Plugin};

pub use error::ConfigError;
pub use types::{MargatroidConfig, WebSocketMessageTarget};

use crate::types::{ConfigDocument, MAX_CONFIG_BYTES};

#[derive(Clone, Debug)]
pub struct ConfigPlugin {
    config: MargatroidConfig,
}

impl ConfigPlugin {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        let source = fs::read_to_string(&path).map_err(|_| ConfigError::ReadFailed(path))?;
        if source.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooLarge);
        }
        let document =
            toml::from_str::<ConfigDocument>(&source).map_err(|_| ConfigError::DecodeFailed)?;
        Ok(Self {
            config: document.try_into()?,
        })
    }

    pub fn new(config: MargatroidConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &MargatroidConfig {
        &self.config
    }
}

impl Plugin for ConfigPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<MargatroidConfig>() {
            panic!("ConfigPlugin is already installed");
        }
        app.world_mut().insert_resource(self.config);
    }
}
