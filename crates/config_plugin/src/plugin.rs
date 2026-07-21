use std::path::PathBuf;

use core_plugin::{App, Plugin, Stage, World};

use crate::events::{ConfigLoadFailed, ConfigLoadRequested, ConfigLoaded, ConfigReloaded};
use crate::resource::ConfigStore;
use crate::systems::{autoload_config, load_requested_configs};

#[derive(Clone, Debug, Default)]
pub struct ConfigPlugin {
    autoload_path: Option<PathBuf>,
}

impl ConfigPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_autoload_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.autoload_path = Some(path.into());
        self
    }
}

impl Plugin for ConfigPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<ConfigLoadRequested>();
        app.add_event::<ConfigLoaded>();
        app.add_event::<ConfigReloaded>();
        app.add_event::<ConfigLoadFailed>();

        if app.world().resource::<ConfigStore>().is_none() {
            app.world_mut().add_resource(ConfigStore::new());
        }

        if let Some(path) = self.autoload_path.clone() {
            let mut path = Some(path);
            app.add_systems(
                Stage::Startup,
                [move |world: &mut World| {
                    if let Some(path) = path.take() {
                        autoload_config(world, path);
                    }
                }],
            );
        }

        let mut reader = app.event_reader::<ConfigLoadRequested>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                load_requested_configs(world, &mut reader);
            }],
        );
    }
}
