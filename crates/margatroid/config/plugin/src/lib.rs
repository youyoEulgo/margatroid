mod handler;

pub use config_core::{ConfigError, MargatroidConfig};

use core_plugin::{App, Plugin};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct ConfigPlugin {
    config: MargatroidConfig,
}

impl ConfigPlugin {
    pub fn new(config: MargatroidConfig) -> Self {
        Self { config }
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        Ok(Self {
            config: handler::read_config(path.as_ref())?,
        })
    }

    pub fn config(&self) -> &MargatroidConfig {
        &self.config
    }
}

impl Plugin for ConfigPlugin {
    fn build(self, app: &mut App) {
        assert!(
            !app.world().contains_resource::<MargatroidConfig>(),
            "ConfigPlugin is already installed"
        );
        app.world_mut().insert_resource(self.config);
    }
}
