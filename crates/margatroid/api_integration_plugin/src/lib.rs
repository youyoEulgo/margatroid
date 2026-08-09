mod logs;
mod state;

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::{App, Plugin, Resource, World};
use log_plugin::TracingStream;

#[derive(Clone, Debug)]
pub struct ApiIntegrationPlugin {
    schedule: String,
    frontend_type: String,
}

impl ApiIntegrationPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            frontend_type: "webui".into(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_frontend_type(mut self, frontend_type: impl Into<String>) -> Self {
        self.frontend_type = frontend_type.into();
        self
    }
}

impl Default for ApiIntegrationPlugin {
    fn default() -> Self {
        Self::new()
    }
}

struct ApiIntegrationPluginInstalled;

impl Resource for ApiIntegrationPluginInstalled {}

impl Plugin for ApiIntegrationPlugin {
    fn build(self, app: &mut App) {
        if app
            .world()
            .contains_resource::<ApiIntegrationPluginInstalled>()
        {
            panic!("ApiIntegrationPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!(
                "ApiIntegrationPlugin schedule does not exist: {}",
                self.schedule
            );
        }
        let stream =
            app.world().get_resource::<TracingStream>().cloned().expect(
                "LogPlugin with a TracingStream must be installed before ApiIntegrationPlugin",
            );
        let events = app.world().event_sender();
        app.world()
            .spawn_async_service(logs::forward_logs(stream, events));
        app.world_mut()
            .insert_resource(ApiIntegrationPluginInstalled);

        let frontend_type = self.frontend_type;
        app.add_system(&self.schedule, logs::report_server_events)
            .add_system(&self.schedule, move |world: &mut World| {
                state::sync_frontend_state_system(world, &frontend_type);
            });
    }
}
