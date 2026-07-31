use std::sync::mpsc::sync_channel;

use core_plugin::{App, Event, Plugin, World};

use crate::resource::RuntimeControl;
use crate::{RuntimeHandle, RuntimeMode};

pub const STARTUP: &str = "startup";
pub const PRE_UPDATE: &str = "pre_update";
pub const UPDATE: &str = "update";
pub const POST_UPDATE: &str = "post_update";

#[derive(Clone, Copy, Debug)]
pub struct RuntimePlugin {
    mode: RuntimeMode,
    frame_rate: Option<u64>,
}

impl RuntimePlugin {
    pub fn fixed(frame_rate: u64) -> Self {
        assert!(
            frame_rate > 0,
            "runtime frame rate must be greater than zero"
        );
        Self {
            mode: RuntimeMode::FixedFrame,
            frame_rate: Some(frame_rate),
        }
    }
}

impl Default for RuntimePlugin {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::EventDriven,
            frame_rate: None,
        }
    }
}

impl Plugin for RuntimePlugin {
    fn build(self, app: &mut App) {
        app.add_once_schedule(STARTUP.into())
            .add_schedule(PRE_UPDATE.into())
            .add_schedule(UPDATE.into())
            .add_schedule(POST_UPDATE.into());

        let (wake_sender, wake_receiver) = sync_channel(1);
        let handle = RuntimeHandle::new(wake_sender);
        let control = RuntimeControl::new(
            self.mode,
            self.frame_rate,
            handle.clone(),
            wake_receiver,
            app.world().event_snapshot(),
        );
        app.world_mut().insert_resource(handle);
        app.world_mut().insert_resource(control);
    }
}

pub trait AppRunExt {
    fn run(&mut self);
}

impl AppRunExt for App {
    fn run(&mut self) {
        let mut runtime = self
            .world_mut()
            .remove_resource::<RuntimeControl>()
            .unwrap_or_else(|| {
                panic!("RuntimePlugin must be installed and App::run may only be called once")
            });
        runtime.run(self);
    }
}

pub trait WorldEventExt {
    fn emit_event<E: Event>(&self, event: E);
    fn emit_event_after<E: Event>(&self, event: E, delay: u64);
}

impl WorldEventExt for World {
    fn emit_event<E: Event>(&self, event: E) {
        let handle = self
            .get_resource::<RuntimeHandle>()
            .unwrap_or_else(|| panic!("RuntimePlugin must be installed before emitting events"))
            .clone();
        self.event_write().send_event(event);
        handle.wake();
    }

    fn emit_event_after<E: Event>(&self, event: E, delay: u64) {
        let handle = self
            .get_resource::<RuntimeHandle>()
            .unwrap_or_else(|| panic!("RuntimePlugin must be installed before emitting events"))
            .clone();
        self.event_write().send_event_after(event, delay);
        handle.wake();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use core_plugin::Event;

    use super::*;

    struct Notice;
    impl Event for Notice {}

    #[test]
    fn default_runtime_is_event_driven() {
        let plugin = RuntimePlugin::default();
        assert_eq!(plugin.mode, RuntimeMode::EventDriven);
        assert_eq!(plugin.frame_rate, None);
    }

    #[test]
    fn event_extensions_send_through_the_core_queue() {
        let mut app = App::new();
        app.register_event::<Notice>()
            .add_plugin(RuntimePlugin::default());

        app.world().emit_event(Notice);

        assert_eq!(app.world().event_snapshot().normal_event_count, 1);
    }

    #[test]
    fn runtime_installs_default_schedules_in_execution_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default());

        for (schedule, label) in [
            (STARTUP, "startup"),
            (PRE_UPDATE, "pre_update"),
            (UPDATE, "update"),
            (POST_UPDATE, "post_update"),
        ] {
            let calls = Arc::clone(&calls);
            app.add_system(schedule, move |_world: &mut World| {
                calls.lock().unwrap().push(label);
            });
        }

        app.tick();
        assert_eq!(
            *calls.lock().unwrap(),
            ["startup", "pre_update", "update", "post_update"]
        );

        calls.lock().unwrap().clear();
        app.tick();
        assert_eq!(
            *calls.lock().unwrap(),
            ["pre_update", "update", "post_update"]
        );
    }

    #[test]
    #[should_panic(expected = "RuntimePlugin must be installed")]
    fn event_extensions_require_the_runtime_plugin() {
        World::new().emit_event(Notice);
    }
}
