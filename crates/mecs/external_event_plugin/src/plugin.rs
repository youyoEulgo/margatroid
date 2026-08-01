use std::marker::PhantomData;

use app_runtime_plugin::AppControl;
use core_plugin::{App, Event, Plugin, Stage, World};
use tokio::sync::mpsc;

use crate::{ExternalEventOptions, ExternalEventSender};

#[derive(Clone, Copy, Debug, Default)]
pub struct ExternalEventPlugin;

#[derive(Clone, Copy, Debug)]
struct ExternalEventPluginInstalled;

struct ExternalEventRegistration<E: Event> {
    options: ExternalEventOptions,
    sender: ExternalEventSender<E>,
    marker: PhantomData<fn() -> E>,
}

impl Plugin for ExternalEventPlugin {
    fn build(&self, app: &mut App) {
        if app
            .world()
            .resource::<ExternalEventPluginInstalled>()
            .is_none()
        {
            app.add_resource(ExternalEventPluginInstalled);
        }
    }
}

pub trait ExternalEventAppExt {
    fn add_external_event<E: Event>(&mut self) -> &mut Self;

    fn add_external_event_with_options<E: Event>(
        &mut self,
        options: ExternalEventOptions,
    ) -> &mut Self;

    fn external_event_sender<E: Event>(&self) -> ExternalEventSender<E>;
}

impl ExternalEventAppExt for App {
    fn add_external_event<E: Event>(&mut self) -> &mut Self {
        self.add_external_event_with_options::<E>(ExternalEventOptions::default())
    }

    fn add_external_event_with_options<E: Event>(
        &mut self,
        options: ExternalEventOptions,
    ) -> &mut Self {
        assert!(
            self.world()
                .resource::<ExternalEventPluginInstalled>()
                .is_some(),
            "ExternalEventPlugin must be installed before registering external events"
        );

        if let Some(registration) = self.world().resource::<ExternalEventRegistration<E>>() {
            assert_eq!(
                registration.options, options,
                "external event is already registered with different options"
            );
            return self;
        }

        self.add_event::<E>();
        let control = self.world().resource::<AppControl>().cloned();
        let (sender, mut receiver) = mpsc::channel::<E>(options.capacity);
        let public_sender = ExternalEventSender::new(sender, control.clone());
        let max_events_per_frame = options.max_events_per_frame;
        self.add_resource(ExternalEventRegistration {
            options,
            sender: public_sender,
            marker: PhantomData,
        });
        self.add_systems(
            Stage::First,
            [move |world: &mut World| {
                for _ in 0..max_events_per_frame {
                    match receiver.try_recv() {
                        Ok(event) => world.emit_event(event),
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => return,
                    }
                }
                if !receiver.is_empty() {
                    if let Some(control) = &control {
                        control.wake();
                    }
                }
            }],
        );
        self
    }

    fn external_event_sender<E: Event>(&self) -> ExternalEventSender<E> {
        self.world()
            .resource::<ExternalEventRegistration<E>>()
            .unwrap_or_else(|| {
                panic!(
                    "external event `{}` is not registered",
                    std::any::type_name::<E>()
                )
            })
            .sender
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use app_runtime_plugin::{AppRunExt, AppRuntimePlugin};
    use core_plugin::EventReader;

    use crate::ExternalEventSendError;

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TestEvent(u32);

    #[derive(Clone)]
    struct EventWithoutDebug;

    #[test]
    fn send_error_is_standard_error_without_event_debug() {
        fn assert_error<T: std::error::Error>() {}
        assert_error::<ExternalEventSendError<EventWithoutDebug>>();
    }

    #[test]
    fn manual_tick_drains_events_in_fifo_order() {
        let mut app = App::new();
        app.add_plugins(ExternalEventPlugin);
        app.add_external_event::<TestEvent>();
        let mut reader = app.event_reader::<TestEvent>();
        let sender = app.external_event_sender::<TestEvent>();

        sender.try_send(TestEvent(1)).unwrap();
        sender.try_send(TestEvent(2)).unwrap();
        app.tick();

        assert_eq!(
            app.world().read_events(&mut reader),
            vec![TestEvent(1), TestEvent(2)]
        );
    }

    #[test]
    fn per_frame_limit_leaves_work_for_next_tick() {
        let mut app = App::new();
        app.add_plugins(ExternalEventPlugin);
        app.add_external_event_with_options::<TestEvent>(
            ExternalEventOptions::default().with_max_events_per_frame(2),
        );
        let mut reader = app.event_reader::<TestEvent>();
        let sender = app.external_event_sender::<TestEvent>();
        for value in 1..=3 {
            sender.try_send(TestEvent(value)).unwrap();
        }

        app.tick();
        assert_eq!(
            app.world().read_events(&mut reader),
            vec![TestEvent(1), TestEvent(2)]
        );
        app.tick();
        assert_eq!(app.world().read_events(&mut reader), vec![TestEvent(3)]);
    }

    #[test]
    fn full_and_closed_errors_return_the_event() {
        let (sender, second_sender) = {
            let mut app = App::new();
            app.add_plugins(ExternalEventPlugin);
            app.add_external_event_with_options::<TestEvent>(
                ExternalEventOptions::default().with_capacity(1),
            );
            let sender = app.external_event_sender::<TestEvent>();
            sender.try_send(TestEvent(1)).unwrap();
            let error = sender.try_send(TestEvent(2)).unwrap_err();
            assert!(matches!(error, ExternalEventSendError::Full(TestEvent(2))));
            let second_sender = sender.clone();
            (sender, second_sender)
        };

        let error = sender.try_send(TestEvent(3)).unwrap_err();
        assert!(matches!(
            error,
            ExternalEventSendError::Closed(TestEvent(3))
        ));
        assert!(sender.is_closed());
        assert!(matches!(
            second_sender.try_send(TestEvent(4)),
            Err(ExternalEventSendError::Closed(TestEvent(4)))
        ));
    }

    #[test]
    fn duplicate_registration_is_idempotent_only_for_equal_options() {
        let mut app = App::new();
        app.add_plugins(ExternalEventPlugin);
        let options = ExternalEventOptions::default().with_capacity(4);
        app.add_external_event_with_options::<TestEvent>(options.clone());
        let mut reader = app.event_reader::<TestEvent>();
        let first = app.external_event_sender::<TestEvent>();
        first.try_send(TestEvent(1)).unwrap();
        app.add_external_event_with_options::<TestEvent>(options);
        let second = app.external_event_sender::<TestEvent>();
        assert_eq!(first.max_capacity(), second.max_capacity());
        app.tick();
        assert_eq!(app.world().read_events(&mut reader), [TestEvent(1)]);

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            app.add_external_event_with_options::<TestEvent>(
                ExternalEventOptions::default().with_capacity(8),
            );
        }));
        assert!(result.is_err());
    }

    #[test]
    fn sender_and_backlog_wake_blocking_app_runtime() {
        let mut app = App::new();
        app.add_plugins(ExternalEventPlugin);
        app.add_plugins(AppRuntimePlugin);
        app.add_external_event_with_options::<TestEvent>(
            ExternalEventOptions::default().with_max_events_per_frame(1),
        );
        let sender = app.external_event_sender::<TestEvent>();
        let control = app.world().resource::<AppControl>().unwrap().clone();
        let seen = Arc::new(AtomicUsize::new(0));
        let system_seen = seen.clone();
        let mut reader: EventReader<TestEvent> = app.event_reader::<TestEvent>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                system_seen.fetch_add(world.read_events(&mut reader).len(), Ordering::SeqCst);
            }],
        );
        let thread = std::thread::spawn(move || app.run());

        sender.try_send(TestEvent(1)).unwrap();
        sender.try_send(TestEvent(2)).unwrap();
        sender.try_send(TestEvent(3)).unwrap();
        wait_until(|| seen.load(Ordering::SeqCst) == 3);
        control.shutdown();
        thread.join().unwrap();
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "condition timed out");
            std::thread::yield_now();
        }
    }
}
