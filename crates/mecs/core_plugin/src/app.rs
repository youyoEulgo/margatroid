use crate::schedule::SchedulePlan;
use crate::{CoreError, Plugin, System, World};

pub struct App {
    world: World,
    schedules: SchedulePlan,
}

impl App {
    pub fn new() -> Self {
        Self {
            world: World::new(),
            schedules: SchedulePlan::new(),
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn add_plugin<P: Plugin>(&mut self, plugin: P) -> &mut Self {
        if self.schedules.is_started() {
            CoreError::AppAlreadyStarted.panic();
        }
        plugin.build(self);
        self
    }

    pub fn add_schedule(&mut self, name: String) -> &mut Self {
        if self.schedules.is_started() {
            CoreError::AppAlreadyStarted.panic();
        }
        let error_name = name.clone();
        if !self.schedules.add_schedule(name) {
            CoreError::ScheduleAlreadyExists { name: error_name }.panic();
        }
        self
    }

    pub fn add_once_schedule(&mut self, name: String) -> &mut Self {
        if self.schedules.is_started() {
            CoreError::AppAlreadyStarted.panic();
        }
        let error_name = name.clone();
        if !self.schedules.add_once_schedule(name) {
            CoreError::ScheduleAlreadyExists { name: error_name }.panic();
        }
        self
    }

    pub fn contains_schedule(&self, name: &str) -> bool {
        self.schedules.contains(name)
    }

    pub fn add_system<S: System>(&mut self, schedule: &str, system: S) -> &mut Self {
        if self.schedules.is_started() {
            CoreError::AppAlreadyStarted.panic();
        }
        self.schedules
            .schedule_mut(schedule)
            .unwrap_or_else(|| {
                CoreError::ScheduleNotFound {
                    name: schedule.into(),
                }
                .panic()
            })
            .add_system(system);
        self
    }

    pub fn tick(&mut self) {
        self.world.tick();
        self.schedules.run(&mut self.world);
    }

    pub fn fast_forward_tick(&mut self) {
        self.world.fast_forward_events();
        self.tick();
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::Event;

    use super::*;

    struct Notice(u32);
    impl Event for Notice {}

    #[test]
    fn events_sent_by_a_system_are_visible_on_the_next_tick() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let reader_seen = Arc::clone(&seen);
        let mut app = App::new();
        app.add_schedule("update".into())
            .add_system("update", move |world: &mut World| {
                let values = world
                    .event_reader::<Notice>()
                    .into_iter()
                    .map(|notice| notice.0)
                    .collect::<Vec<_>>();
                reader_seen.lock().unwrap().extend(values);
                world.emit_event(Notice(7));
            });

        app.tick();
        assert!(seen.lock().unwrap().is_empty());
        app.tick();
        assert_eq!(*seen.lock().unwrap(), [7]);
    }

    #[test]
    fn reading_an_event_type_before_its_first_event_is_empty() {
        let app = App::new();

        assert!(app.world().event_reader::<Notice>().is_empty());
    }

    #[test]
    #[should_panic(expected = "app has already started")]
    fn plugins_cannot_be_added_after_startup() {
        let mut app = App::new();
        app.tick();
        app.add_plugin(|_app: &mut App| {});
    }

    #[test]
    fn delayed_events_arrive_after_their_countdown_and_then_clear() {
        let mut app = App::new();
        app.world().emit_event_after(Notice(3), 2);

        app.tick();
        assert!(app.world().event_reader::<Notice>().is_empty());
        app.tick();
        assert!(app.world().event_reader::<Notice>().is_empty());
        app.tick();
        assert_eq!(
            app.world()
                .event_reader::<Notice>()
                .into_iter()
                .map(|notice| notice.0)
                .collect::<Vec<_>>(),
            [3]
        );
        app.tick();
        assert!(app.world().event_reader::<Notice>().is_empty());
    }

    #[test]
    fn maximum_delay_counts_down_without_overflowing() {
        let mut app = App::new();
        app.world().emit_event_after(Notice(1), u64::MAX);

        app.tick();

        assert!(app.world().event_reader::<Notice>().is_empty());
    }

    #[test]
    fn first_event_automatically_creates_its_read_storage() {
        let mut app = App::new();
        app.world().emit_event(Notice(5));

        app.tick();

        assert_eq!(app.world().event_reader::<Notice>().len(), 1);
    }

    #[test]
    fn schedule_presence_can_be_queried_without_mutating_the_plan() {
        let mut app = App::new();
        app.add_once_schedule("startup".into())
            .add_schedule("update".into());

        assert!(app.contains_schedule("startup"));
        assert!(app.contains_schedule("update"));
        assert!(!app.contains_schedule("missing"));
    }

    #[test]
    fn pending_events_remain_queued_until_completed() {
        let mut app = App::new();
        let _handle = app.world().emit_pending::<u32, String>();

        app.tick();

        let snapshot = app.world().event_snapshot();
        assert_eq!(snapshot.normal_event_count, 0);
        assert_eq!(snapshot.pending_event_count, 1);
        assert_eq!(snapshot.nearest_normal_event_delay, None);
        assert!(app.world().event_reader::<Result<u32, String>>().is_empty());
    }

    #[test]
    fn pending_events_can_be_completed_from_another_thread() {
        let mut app = App::new();
        let handle = app.world().emit_pending::<u32, String>();

        std::thread::spawn(move || handle.complete(Ok(7)))
            .join()
            .unwrap();

        let snapshot = app.world().event_snapshot();
        assert_eq!(snapshot.normal_event_count, 1);
        assert_eq!(snapshot.pending_event_count, 0);
        assert_eq!(snapshot.nearest_normal_event_delay, Some(0));

        app.tick();

        let reader = app.world().event_reader::<Result<u32, String>>();
        assert!(matches!(reader.into_iter().next(), Some(Ok(7))));
        let snapshot = app.world().event_snapshot();
        assert_eq!(snapshot.normal_event_count, 0);
        assert_eq!(snapshot.pending_event_count, 0);
        assert_eq!(snapshot.nearest_normal_event_delay, None);
    }

    #[test]
    fn fast_forward_tick_delivers_a_delayed_completed_event() {
        let mut app = App::new();
        let handle = app.world().emit_pending::<u32, String>();
        handle.complete_after(Ok(11), 5);

        app.fast_forward_tick();

        let reader = app.world().event_reader::<Result<u32, String>>();
        assert!(matches!(reader.into_iter().next(), Some(Ok(11))));
        assert_eq!(app.world().event_snapshot().normal_event_count, 0);
    }

    #[test]
    #[should_panic(expected = "schedule `update` already exists")]
    fn schedule_names_must_be_unique_across_once_and_recurring_schedules() {
        let mut app = App::new();
        app.add_schedule("update".into());
        app.add_once_schedule("update".into());
    }
}
