use std::collections::HashSet;
use std::sync::Mutex;

use app_runtime_plugin::{RuntimeHandle, WorldEventExt};
use core_plugin::{App, Event, Plugin, Resource, System, World};

use crate::ClosureError;

type ErasedClosure = Box<dyn FnOnce(&mut World) + Send + 'static>;

struct ClosureRegistry {
    schedules: HashSet<String>,
}

impl ClosureRegistry {
    fn new() -> Self {
        Self {
            schedules: HashSet::new(),
        }
    }

    fn register(&mut self, schedule: String) -> bool {
        self.schedules.insert(schedule)
    }

    fn contains(&self, schedule: &str) -> bool {
        self.schedules.contains(schedule)
    }
}

impl Resource for ClosureRegistry {}

struct ClosureRequest {
    target_schedule: String,
    closure: Mutex<Option<ErasedClosure>>,
}

impl ClosureRequest {
    fn new(target_schedule: String, closure: ErasedClosure) -> Self {
        Self {
            target_schedule,
            closure: Mutex::new(Some(closure)),
        }
    }

    fn take_for(&self, schedule: &str) -> Option<ErasedClosure> {
        if self.target_schedule != schedule {
            return None;
        }
        self.closure
            .lock()
            .expect("closure request lock poisoned")
            .take()
    }
}

impl Event for ClosureRequest {}

struct ClosureSystem {
    schedule: String,
}

impl ClosureSystem {
    fn new(schedule: String) -> Self {
        Self { schedule }
    }
}

impl System for ClosureSystem {
    fn run(&mut self, world: &mut World) {
        let closures = world
            .event_reader::<ClosureRequest>()
            .into_iter()
            .filter_map(|request| request.take_for(&self.schedule))
            .collect::<Vec<_>>();

        for closure in closures {
            closure(world);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClosurePlugin;

impl Plugin for ClosurePlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            ClosureError::RuntimePluginMissing.panic();
        }
        if app.world().contains_resource::<ClosureRegistry>() {
            ClosureError::ClosurePluginAlreadyInstalled.panic();
        }
        app.world_mut().insert_resource(ClosureRegistry::new());
    }
}

pub trait AppClosureExt {
    fn add_closure_system(&mut self, schedule: &str) -> &mut Self;
}

impl AppClosureExt for App {
    fn add_closure_system(&mut self, schedule: &str) -> &mut Self {
        let Some(registry) = self.world_mut().get_resource_mut::<ClosureRegistry>() else {
            ClosureError::ClosurePluginMissing.panic();
        };
        if !registry.register(schedule.into()) {
            ClosureError::ClosureSystemAlreadyRegistered {
                schedule: schedule.into(),
            }
            .panic();
        }

        self.add_system(schedule, ClosureSystem::new(schedule.into()))
    }
}

pub trait WorldClosureExt {
    fn send_closure<Closure>(&self, schedule: &str, closure: Closure)
    where
        Closure: FnOnce(&mut World) + Send + 'static;
}

impl WorldClosureExt for World {
    fn send_closure<Closure>(&self, schedule: &str, closure: Closure)
    where
        Closure: FnOnce(&mut World) + Send + 'static,
    {
        let Some(registry) = self.get_resource::<ClosureRegistry>() else {
            ClosureError::ClosurePluginMissing.panic();
        };
        if !registry.contains(schedule) {
            ClosureError::ClosureSystemNotRegistered {
                schedule: schedule.into(),
            }
            .panic();
        }

        WorldEventExt::send_event(
            self,
            ClosureRequest::new(schedule.into(), Box::new(closure)),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use app_runtime_plugin::RuntimePlugin;

    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE);
        app
    }

    #[test]
    fn closure_runs_in_the_selected_schedule_with_world_access() {
        struct Value(u32);
        impl Resource for Value {}

        let mut app = app();
        app.world_mut().insert_resource(Value(1));
        app.world().send_closure(RuntimePlugin::UPDATE, |world| {
            world.get_resource_mut::<Value>().unwrap().0 = 2;
        });

        app.tick();

        assert_eq!(app.world().get_resource::<Value>().unwrap().0, 2);
    }

    #[test]
    fn closure_only_runs_in_its_target_schedule() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut app = app();
        app.add_closure_system(RuntimePlugin::POST_UPDATE);
        let closure_calls = Arc::clone(&calls);
        app.world()
            .send_closure(RuntimePlugin::POST_UPDATE, move |_world| {
                closure_calls.lock().unwrap().push("post_update");
            });

        app.tick();

        assert_eq!(*calls.lock().unwrap(), ["post_update"]);
    }

    #[test]
    #[should_panic(expected = "RuntimePlugin is not installed")]
    fn runtime_plugin_must_be_installed_first() {
        App::new().add_plugin(ClosurePlugin);
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn duplicate_closure_system_is_rejected() {
        app().add_closure_system(RuntimePlugin::UPDATE);
    }

    #[test]
    #[should_panic(expected = "no ClosureSystem is registered")]
    fn sending_to_an_unregistered_schedule_is_rejected() {
        let mut app = app();
        app.add_schedule("custom".into());
        app.world().send_closure("custom", |_world| {});
    }
}
