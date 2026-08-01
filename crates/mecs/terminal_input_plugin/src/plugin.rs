use app_runtime_plugin::{AppControl, AppShutdownExt};
use core_plugin::{App, Plugin, Stage, World};
use external_event_plugin::{ExternalEventAppExt, ExternalEventOptions};

use crate::{TerminalEvent, TerminalInputFailed, TerminalInputOptions, TerminalSessionHandle};

#[derive(Clone, Debug)]
pub struct TerminalInputPlugin {
    options: TerminalInputOptions,
}

impl TerminalInputPlugin {
    pub fn with_options(options: TerminalInputOptions) -> Self {
        Self { options }
    }
}

impl Plugin for TerminalInputPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().resource::<TerminalSessionHandle>().is_none(),
            "TerminalInputPlugin can only be installed once"
        );
        let external_options =
            ExternalEventOptions::default().with_capacity(self.options.capacity());
        app.add_external_event_with_options::<TerminalEvent>(external_options.clone());
        app.add_external_event_with_options::<TerminalInputFailed>(external_options);
        let event_sender = app.external_event_sender::<TerminalEvent>();
        let failure_sender = app.external_event_sender::<TerminalInputFailed>();
        let handle = TerminalSessionHandle::new();
        app.add_resource(handle.clone());

        let options = self.options.clone();
        let startup_handle = handle.clone();
        app.add_systems(
            Stage::Startup,
            [move |world: &mut World| {
                if let Err(failure) = startup_handle.start(
                    options.clone(),
                    event_sender.clone(),
                    failure_sender.clone(),
                ) {
                    world.emit_event(failure);
                }
            }],
        );

        if app.world().resource::<AppControl>().is_some() {
            app.on_shutdown(move |_world| {
                handle.shutdown();
            });
        }
    }
}
