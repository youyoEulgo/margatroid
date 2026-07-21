use app_runtime_plugin::AppControl;
use core_plugin::{App, Plugin, Stage, World};

use crate::events::{
    HttpRequestReceived, ServerFailed, ServerStartRequested, ServerStarted, ShutdownRequested,
    UserPromptSubmitted,
};
use crate::resource::{ServerConfig, ServerHandle};
use crate::systems::{handle_server_start_requests, handle_shutdown_requests, start_server};

#[derive(Clone, Debug)]
pub struct ServerPlugin {
    config: ServerConfig,
    auto_start: bool,
}

impl ServerPlugin {
    pub fn new() -> Self {
        Self {
            config: ServerConfig::default(),
            auto_start: false,
        }
    }

    pub fn with_config(mut self, config: ServerConfig) -> Self {
        self.config = config;
        self
    }

    pub fn auto_start(mut self) -> Self {
        self.auto_start = true;
        self
    }
}

impl Default for ServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().resource::<AppControl>().is_some(),
            "AppRuntimePlugin must be installed before ServerPlugin"
        );
        app.add_event::<ServerStartRequested>();
        app.add_event::<ServerStarted>();
        app.add_event::<ServerFailed>();
        app.add_event::<ShutdownRequested>();
        app.add_event::<HttpRequestReceived>();
        app.add_event::<UserPromptSubmitted>();

        if app.world().resource::<ServerConfig>().is_none() {
            app.world_mut().add_resource(self.config.clone());
        }
        if app.world().resource::<ServerHandle>().is_none() {
            app.world_mut().add_resource(ServerHandle::new());
        }

        if self.auto_start {
            let mut started = false;
            app.add_systems(
                Stage::Startup,
                [move |world: &mut World| {
                    if !started {
                        started = true;
                        start_server(world);
                    }
                }],
            );
        }

        let mut start_reader = app.event_reader::<ServerStartRequested>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                handle_server_start_requests(world, &mut start_reader);
            }],
        );

        let control = app.world().resource::<AppControl>().unwrap().clone();
        let mut shutdown_reader = app.event_reader::<ShutdownRequested>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                handle_shutdown_requests(world, &mut shutdown_reader, &control);
            }],
        );
    }
}
