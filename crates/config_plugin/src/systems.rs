use std::path::PathBuf;

use core_plugin::{EventReader, World};
use types::config::AppConfig;

use crate::events::{ConfigLoadFailed, ConfigLoadRequested, ConfigLoaded, ConfigReloaded};
use crate::resource::ConfigStore;

pub(crate) fn autoload_config(world: &mut World, path: PathBuf) {
    match load_config_file(&path) {
        Ok(config) => {
            world
                .resource::<ConfigStore>()
                .expect("ConfigStore should be registered by ConfigPlugin")
                .set(path.clone(), config.clone());
            world.send_event(ConfigLoaded { path, config });
        }
        Err(message) => world.send_event(ConfigLoadFailed { path, message }),
    }
}

pub(crate) fn load_requested_configs(
    world: &mut World,
    reader: &mut EventReader<ConfigLoadRequested>,
) {
    for request in world.read_events(reader) {
        match load_config_file(&request.path) {
            Ok(config) => {
                let previous = world
                    .resource::<ConfigStore>()
                    .expect("ConfigStore should be registered by ConfigPlugin")
                    .set(request.path.clone(), config.clone());
                if previous.is_some() {
                    world.send_event(ConfigReloaded {
                        path: request.path,
                        config,
                    });
                } else {
                    world.send_event(ConfigLoaded {
                        path: request.path,
                        config,
                    });
                }
            }
            Err(message) => world.send_event(ConfigLoadFailed {
                path: request.path,
                message,
            }),
        }
    }
}

fn load_config_file(path: &PathBuf) -> Result<AppConfig, String> {
    let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    toml::from_str(&content).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use core_plugin::{App, Stage, World};
    use types::config::AppConfig;

    use crate::{ConfigLoadFailed, ConfigLoadRequested, ConfigLoaded, ConfigPlugin, ConfigStore};

    #[test]
    fn plugin_loads_config_from_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = AppConfig::default();
        config.app.name = "Test".into();
        std::fs::write(&path, toml::to_string_pretty(&config).unwrap()).unwrap();

        let mut app = App::new();
        app.add_plugins(ConfigPlugin::new());

        let loaded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let system_loaded = loaded.clone();
        let mut reader = app.event_reader::<ConfigLoaded>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                system_loaded
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );

        app.world().send_event(ConfigLoadRequested::new(&path));
        app.tick();

        assert_eq!(loaded.lock().unwrap().len(), 1);
        assert_eq!(
            app.world()
                .resource::<ConfigStore>()
                .unwrap()
                .config()
                .unwrap()
                .app
                .name,
            "Test"
        );
    }

    #[test]
    fn plugin_reports_load_failure() {
        let mut app = App::new();
        app.add_plugins(ConfigPlugin::new());

        let failures = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let system_failures = failures.clone();
        let mut reader = app.event_reader::<ConfigLoadFailed>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                system_failures
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );

        app.world()
            .send_event(ConfigLoadRequested::new("missing-config.toml"));
        app.tick();

        assert_eq!(failures.lock().unwrap().len(), 1);
    }
}
