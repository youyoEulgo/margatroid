use async_runtime_plugin::{AsyncAppExt, AsyncSystemOptions};
use core_plugin::{App, Plugin, Stage, World};

use crate::events::{
    SandboxCommandCompleted, SandboxCommandFailed, SandboxCommandRequested, SandboxCommandStarted,
};
use crate::resource::{SandboxExecutor, SandboxPolicy};
use crate::systems::{execute_sandbox_command, SandboxAsyncOutput};

#[derive(Clone, Debug, Default)]
pub struct SandboxPluginOptions {
    pub policy: SandboxPolicy,
}

#[derive(Clone, Debug, Default)]
pub struct SandboxPlugin {
    options: SandboxPluginOptions,
}

impl SandboxPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_policy(mut self, policy: SandboxPolicy) -> Self {
        self.options.policy = policy;
        self
    }
}

impl Plugin for SandboxPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<SandboxCommandRequested>();
        app.add_event::<SandboxCommandStarted>();
        app.add_event::<SandboxCommandCompleted>();
        app.add_event::<SandboxCommandFailed>();

        if app.world().resource::<SandboxPolicy>().is_none() {
            app.world_mut().add_resource(self.options.policy.clone());
        }
        if app.world().resource::<SandboxExecutor>().is_none() {
            app.world_mut().add_resource(SandboxExecutor::new());
        }

        let policy = app
            .world()
            .resource::<SandboxPolicy>()
            .expect("SandboxPolicy should be registered by SandboxPlugin")
            .clone();

        let mut started_reader = app.event_reader::<SandboxCommandRequested>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                for request in world.read_events(&mut started_reader) {
                    world.send_event(SandboxCommandStarted {
                        command_id: request.command_id,
                    });
                }
            }],
        );

        app.add_async_system_with_options(
            move |request: SandboxCommandRequested| {
                let policy = policy.clone();
                async move { execute_sandbox_command(policy, request).await }
            },
            AsyncSystemOptions { timeout: None },
        );

        let mut reader = app.event_reader::<SandboxAsyncOutput>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                for output in world.read_events(&mut reader) {
                    match output {
                        SandboxAsyncOutput::Completed(event) => world.send_event(event),
                        SandboxAsyncOutput::Failed(event) => world.send_event(event),
                    }
                }
            }],
        );
    }
}
