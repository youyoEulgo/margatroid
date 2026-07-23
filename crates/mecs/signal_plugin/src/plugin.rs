use app_runtime_plugin::{AppControl, AppShutdownExt};
use core_plugin::{App, Plugin, Stage, World};
use external_event_plugin::{ExternalEventAppExt, ExternalEventOptions};

use crate::{ProcessSignalReceived, SignalHandle, SignalListenerFailed, SignalOptions};

#[derive(Clone, Debug, Default)]
pub struct SignalPlugin {
    options: SignalOptions,
}

impl SignalPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(options: SignalOptions) -> Self {
        Self { options }
    }
}

impl Plugin for SignalPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().resource::<SignalHandle>().is_none(),
            "SignalPlugin can only be installed once"
        );

        app.add_event::<SignalListenerFailed>();
        app.add_external_event_with_options::<ProcessSignalReceived>(
            ExternalEventOptions::default().with_capacity(self.options.capacity()),
        );
        let sender = app.external_event_sender::<ProcessSignalReceived>();
        let handle = SignalHandle::new();
        app.add_resource(handle.clone());

        let options = self.options.clone();
        let startup_handle = handle.clone();
        app.add_systems(
            Stage::Startup,
            [move |world: &mut World| {
                if let Err(error) = startup_handle.start(options.signals(), sender.clone()) {
                    world.send_event(SignalListenerFailed {
                        message: error.to_string(),
                    });
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use core_plugin::EventReader;
    use external_event_plugin::ExternalEventPlugin;

    use super::*;
    use crate::ProcessSignal;

    #[test]
    fn signal_becomes_event_without_app_runtime_or_implicit_shutdown() {
        let mut app = App::new();
        app.add_plugins(ExternalEventPlugin);
        app.add_plugins(SignalPlugin::with_options(
            SignalOptions::new().with_signals([ProcessSignal::User1]),
        ));
        let mut reader: EventReader<ProcessSignalReceived> = app.event_reader();
        app.tick();

        signal_hook::low_level::raise(signal_hook::consts::SIGUSR1).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            let events = app.world().read_events(&mut reader);
            if !events.is_empty() {
                assert_eq!(events[0].signal, ProcessSignal::User1);
                break;
            }
            assert!(Instant::now() < deadline, "signal event timed out");
            std::thread::yield_now();
        }

        assert!(app.world().resource::<AppControl>().is_none());
        app.world().resource::<SignalHandle>().unwrap().shutdown();
    }
}
