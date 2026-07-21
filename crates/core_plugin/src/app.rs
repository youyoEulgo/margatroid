use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use crate::events::{Event, EventReader, Events};
use crate::plugin::PluginGroup;
use crate::resource::Resource;
use crate::schedule::{Schedule, ScheduleReport};
use crate::system::{System, SystemFailed};
use crate::world::World;

/// Core 只定义通用帧阶段，不携带业务语义。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stage {
    Startup,
    First,
    Update,
    Last,
}

/// 同步 ECS 组合根。持有 World 和按阶段分组的 Schedule。
pub struct App {
    world: World,
    schedules: HashMap<Stage, Schedule>,
    event_maintenance: Schedule,
    event_types: HashSet<TypeId>,
    started: bool,
    event_retention_frames: u64,
}

impl App {
    pub fn new() -> Self {
        let schedules = [Stage::Startup, Stage::First, Stage::Update, Stage::Last]
            .into_iter()
            .map(|stage| (stage, Schedule::new()))
            .collect();
        Self {
            world: World::new(),
            schedules,
            event_maintenance: Schedule::new(),
            event_types: HashSet::new(),
            started: false,
            event_retention_frames: 2,
        }
    }

    pub fn add_plugins(&mut self, plugins: impl Into<PluginGroup>) -> &mut Self {
        plugins.into().build_all(self);
        self
    }

    pub fn add_systems(
        &mut self,
        stage: Stage,
        systems: impl IntoIterator<Item = impl System>,
    ) -> &mut Self {
        let schedule = self
            .schedules
            .get_mut(&stage)
            .expect("stage not registered");
        for system in systems {
            schedule.add_system(system);
        }
        self
    }

    pub fn add_resource<R: Resource>(&mut self, resource: R) -> &mut Self {
        self.world.add_resource(resource);
        self
    }

    /// 注册一种事件类型。重复注册不会重置已有队列。
    pub fn add_event<E: Event>(&mut self) -> &mut Self {
        if self.world.resource::<Events<E>>().is_none() {
            self.world
                .add_resource(Events::<E>::new(self.event_retention_frames));
        }
        if self.event_types.insert(TypeId::of::<E>()) {
            self.event_maintenance.add_system(|world: &mut World| {
                if let Some(events) = world.resource::<Events<E>>() {
                    events.finish_frame();
                }
            });
        }
        self
    }

    /// 为已注册的事件类型创建独立 reader。
    pub fn event_reader<E: Event>(&self) -> EventReader<E> {
        self.world
            .resource::<Events<E>>()
            .unwrap_or_else(|| panic!("event `{}` is not registered", std::any::type_name::<E>()))
            .reader()
    }

    /// 设置事件保留帧数。必须在注册首个事件类型前调用。
    pub fn set_event_retention_frames(&mut self, frames: u64) -> &mut Self {
        assert!(frames > 0, "event retention must be positive");
        assert!(
            self.event_types.is_empty(),
            "event retention cannot change after events are registered"
        );
        self.event_retention_frames = frames;
        self
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn schedule_mut(&mut self, stage: Stage) -> &mut Schedule {
        self.schedules
            .get_mut(&stage)
            .expect("stage not registered")
    }

    /// 执行一个同步 ECS 帧。第一次调用时先运行一次 Startup。
    pub fn tick(&mut self) {
        self.add_event::<SystemFailed>();
        if !self.started {
            self.run_stage(Stage::Startup);
            self.started = true;
        }
        for stage in [Stage::First, Stage::Update, Stage::Last] {
            self.run_stage(stage);
        }
        let report = self.event_maintenance.run(&mut self.world);
        self.handle_schedule_report("EventMaintenance", report);
    }

    fn run_stage(&mut self, stage: Stage) {
        let report = self
            .schedules
            .get_mut(&stage)
            .expect("stage not registered")
            .run(&mut self.world);
        self.handle_schedule_report(stage.name(), report);
    }

    fn handle_schedule_report(&mut self, schedule: &'static str, report: ScheduleReport) {
        if let Some(message) = report.ordering_error {
            tracing::error!(schedule, %message, "schedule configuration failed");
            self.world.send_event(SystemFailed {
                schedule,
                system: None,
                message,
            });
        }
        for failure in report.failures {
            tracing::error!(
                schedule,
                system = failure.system.unwrap_or("<anonymous>"),
                message = %failure.message,
                "system execution failed"
            );
            self.world.send_event(SystemFailed {
                schedule,
                system: failure.system,
                message: failure.message,
            });
        }
    }
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Startup => "Startup",
            Stage::First => "First",
            Stage::Update => "Update",
            Stage::Last => "Last",
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::named_system;

    #[derive(Debug, PartialEq)]
    struct Counter(i32);

    #[test]
    fn stages_run_in_core_order() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut app = App::new();
        for (stage, value) in [
            (Stage::Startup, 0),
            (Stage::First, 1),
            (Stage::Update, 2),
            (Stage::Last, 3),
        ] {
            let calls = calls.clone();
            app.add_systems(
                stage,
                [move |_world: &mut World| {
                    calls.lock().unwrap().push(value);
                }],
            );
        }

        app.tick();
        app.tick();

        assert_eq!(*calls.lock().unwrap(), [0, 1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn resources_can_be_added_through_app() {
        let mut app = App::new();
        app.add_resource(Counter(7));
        assert_eq!(app.world().resource::<Counter>(), Some(&Counter(7)));
    }

    #[test]
    fn ordering_failures_become_events() {
        let mut app = App::new();
        let mut reader = {
            app.add_event::<SystemFailed>();
            app.event_reader::<SystemFailed>()
        };
        app.add_systems(
            Stage::Update,
            [named_system("first", |_world| {}).after("missing")],
        );

        app.tick();

        let failures = app.world().read_events(&mut reader);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].message.contains("unknown system label"));
    }
}
