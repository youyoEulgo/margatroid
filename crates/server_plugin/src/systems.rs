use app_runtime_plugin::AppControl;
use core_plugin::{EventReader, World};

use crate::events::{ServerFailed, ServerStartRequested, ServerStarted, ShutdownRequested};
use crate::resource::{ServerConfig, ServerHandle};

pub(crate) fn start_server(world: &mut World) {
    let config = world
        .resource::<ServerConfig>()
        .expect("ServerConfig should be registered by ServerPlugin")
        .clone();
    let result = world
        .resource::<ServerHandle>()
        .expect("ServerHandle should be registered by ServerPlugin")
        .start(&config);

    match result {
        Ok(address) => world.send_event(ServerStarted { address }),
        Err(message) => world.send_event(ServerFailed { message }),
    }
}

pub(crate) fn handle_server_start_requests(
    world: &mut World,
    reader: &mut EventReader<ServerStartRequested>,
) {
    for _ in world.read_events(reader) {
        start_server(world);
    }
}

pub(crate) fn handle_shutdown_requests(
    world: &mut World,
    reader: &mut EventReader<ShutdownRequested>,
    control: &AppControl,
) {
    if world.read_events(reader).is_empty() {
        return;
    }
    if let Some(handle) = world.resource::<ServerHandle>() {
        handle.shutdown();
    }
    control.shutdown();
}

#[cfg(test)]
mod tests {
    use app_runtime_plugin::AppRuntimePlugin;
    use core_plugin::App;

    use crate::{ServerConfig, ServerFailed, ServerPlugin, ServerStartRequested, ServerStarted};

    #[test]
    fn plugin_reports_server_start_result() {
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        app.add_plugins(ServerPlugin::new().with_config(ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
        }));

        let mut reader = app.event_reader::<ServerStarted>();
        let mut failed_reader = app.event_reader::<ServerFailed>();
        app.world().send_event(ServerStartRequested);
        app.tick();

        let started = app.world().read_events(&mut reader);
        let failed = app.world().read_events(&mut failed_reader);
        assert_eq!(started.len() + failed.len(), 1);
    }
}
