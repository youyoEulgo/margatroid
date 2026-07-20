use std::time::Duration;

use core_plugin::{App, AsyncSystemOptions, Plugin};

use crate::events::{LlmFailed, LlmRequest, LlmResponse, LlmStreamChunk};
use crate::resource::LlmProviderRegistry;
use crate::systems::{handle_llm_request, LlmAsyncBatch, LlmAsyncOutput};

#[derive(Clone, Copy, Debug)]
pub struct LlmPluginOptions {
    pub timeout: Option<Duration>,
}

impl Default for LlmPluginOptions {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(60)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LlmPlugin {
    options: LlmPluginOptions,
}

impl LlmPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    pub fn without_timeout(mut self) -> Self {
        self.options.timeout = None;
        self
    }
}

impl Plugin for LlmPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<LlmRequest>();
        app.add_event::<LlmResponse>();
        app.add_event::<LlmStreamChunk>();
        app.add_event::<LlmFailed>();

        if app.world().resource::<LlmProviderRegistry>().is_none() {
            app.world_mut().add_resource(LlmProviderRegistry::new());
        }

        let registry = app
            .world()
            .resource::<LlmProviderRegistry>()
            .expect("LlmProviderRegistry should be registered by LlmPlugin")
            .clone();
        app.add_async_system_with_options(
            move |request: LlmRequest| {
                let registry = registry.clone();
                async move { handle_llm_request(registry, request).await }
            },
            AsyncSystemOptions {
                timeout: self.options.timeout,
            },
        );

        let mut reader = app.event_reader::<LlmAsyncBatch>();
        app.add_systems(
            core_plugin::Stage::Finalize,
            [move |world: &mut core_plugin::World| {
                for batch in world.read_events(&mut reader) {
                    for output in batch.outputs {
                        match output {
                            LlmAsyncOutput::Response(event) => world.send_event(event),
                            LlmAsyncOutput::StreamChunk(event) => world.send_event(event),
                            LlmAsyncOutput::Failed(event) => world.send_event(event),
                        }
                    }
                }
            }],
        );
    }
}
