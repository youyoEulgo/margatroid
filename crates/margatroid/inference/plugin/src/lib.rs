mod handler;
mod system;
mod types;

use std::path::Path;
use std::time::Duration;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AsyncRuntimeHandle, WorldAsyncExt};
use core_plugin::{App, Plugin};
use inference_core::{InferenceError, ModelRoutes};

use types::HttpRuntime;

pub struct InferencePlugin {
    schedule: String,
    routes: ModelRoutes,
}

impl InferencePlugin {
    pub fn open(path: &Path) -> Result<Self, InferenceError> {
        Ok(Self {
            schedule: RuntimePlugin::UPDATE.into(),
            routes: handler::load_model_routes(path)?,
        })
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Plugin for InferencePlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>(),
            "RuntimePlugin required by InferencePlugin"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AsyncRuntimePlugin required by InferencePlugin"
        );
        assert!(
            !app.world().contains_resource::<HttpRuntime>(),
            "InferencePlugin already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "InferencePlugin schedule missing"
        );
        assert!(
            self.schedule != RuntimePlugin::STARTUP,
            "InferencePlugin requires a recurring schedule"
        );
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        app.world().spawn_async_service(async move {
            let _ = sender.send(tokio::runtime::Handle::current());
        });
        let executor = receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("inference async runtime handshake failed");
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("inference HTTP client creation failed");
        app.world_mut().insert_resource(self.routes);
        app.world_mut()
            .insert_resource(HttpRuntime { client, executor });
        app.add_system(&self.schedule, system::init_inference_system);
        app.add_system(&self.schedule, system::inference_system);
    }
}
