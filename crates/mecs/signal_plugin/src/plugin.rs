use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Plugin, World};

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
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>(),
            "RuntimePlugin must be installed before SignalPlugin"
        );
        assert!(
            !app.world().contains_resource::<SignalHandle>(),
            "SignalPlugin can only be installed once"
        );

        let handle = SignalHandle::new();
        let startup_handle = handle.clone();
        let signals = self.options.signals;
        app.world_mut().insert_resource(handle);
        app.register_event::<ProcessSignalReceived>()
            .register_event::<SignalListenerFailed>()
            .add_system(RuntimePlugin::STARTUP, move |world: &mut World| {
                if let Err(error) = startup_handle.start(&signals, world.event_sender()) {
                    world.send_event(SignalListenerFailed {
                        message: error.to_string(),
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::ProcessSignal;

    #[test]
    fn signal_becomes_event_without_implicit_shutdown() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(SignalPlugin::with_options(
                SignalOptions::new().with_signals([ProcessSignal::User1]),
            ));
        app.tick();

        signal_hook::low_level::raise(signal_hook::consts::SIGUSR1).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(event) = app
                .world()
                .event_reader::<ProcessSignalReceived>()
                .into_iter()
                .next()
            {
                assert_eq!(event.signal, ProcessSignal::User1);
                break;
            }
            assert!(Instant::now() < deadline, "signal event timed out");
            std::thread::yield_now();
        }

        app.world()
            .get_resource::<SignalHandle>()
            .unwrap()
            .shutdown();
    }
}
