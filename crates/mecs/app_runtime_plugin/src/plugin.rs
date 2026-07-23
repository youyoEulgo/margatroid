use core_plugin::{App, Plugin, World};

use crate::shutdown::ShutdownSystems;
use crate::{AppControl, ShutdownPhase};

#[derive(Clone, Copy, Debug, Default)]
pub struct AppRuntimePlugin;

impl Plugin for AppRuntimePlugin {
    fn build(&self, app: &mut App) {
        if app.world().resource::<AppControl>().is_none() {
            app.add_resource(AppControl::new());
        }
        if app.world().resource::<ShutdownSystems>().is_none() {
            app.add_resource(ShutdownSystems::new());
        }
    }
}

pub trait AppRunExt {
    /// 持续执行同步帧；没有 wake 或 shutdown 请求时阻塞当前线程。
    fn run(&mut self);
}

impl AppRunExt for App {
    fn run(&mut self) {
        let control = self
            .world()
            .resource::<AppControl>()
            .unwrap_or_else(|| panic!("AppRuntimePlugin must be installed before App::run()"))
            .clone();
        while !control.is_shutdown() {
            self.tick();
            control.wait();
        }
        let systems = self
            .world()
            .resource::<ShutdownSystems>()
            .expect("AppRuntimePlugin shutdown registry should be installed")
            .clone();
        systems.run(self.world_mut());
    }
}

pub trait AppShutdownExt {
    fn add_shutdown_system(
        &mut self,
        phase: ShutdownPhase,
        system: impl FnMut(&mut World) + Send + 'static,
    ) -> &mut Self;
}

impl AppShutdownExt for App {
    fn add_shutdown_system(
        &mut self,
        phase: ShutdownPhase,
        system: impl FnMut(&mut World) + Send + 'static,
    ) -> &mut Self {
        self.world_mut()
            .resource::<ShutdownSystems>()
            .unwrap_or_else(|| panic!("AppRuntimePlugin must be installed before shutdown systems"))
            .add(phase, system);
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use core_plugin::{Stage, World};

    use super::*;

    #[test]
    fn run_waits_for_wake_and_stops_on_shutdown() {
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        let ticks = Arc::new(AtomicUsize::new(0));
        let system_ticks = ticks.clone();
        app.add_systems(
            Stage::Update,
            [move |_world: &mut World| {
                system_ticks.fetch_add(1, Ordering::SeqCst);
            }],
        );
        let control = app.world().resource::<AppControl>().unwrap().clone();
        let thread = std::thread::spawn(move || app.run());

        wait_until(|| ticks.load(Ordering::SeqCst) == 1);
        control.wake();
        wait_until(|| ticks.load(Ordering::SeqCst) == 2);
        control.shutdown();
        thread.join().unwrap();

        assert_eq!(ticks.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn shutdown_systems_run_in_phase_order_and_isolate_panics() {
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        for (phase, value) in [
            (ShutdownPhase::Finish, 5),
            (ShutdownPhase::StopWorkers, 3),
            (ShutdownPhase::Begin, 1),
            (ShutdownPhase::StopIngress, 2),
            (ShutdownPhase::FlushState, 4),
        ] {
            let calls = calls.clone();
            app.add_shutdown_system(phase, move |_world| calls.lock().unwrap().push(value));
        }
        app.add_shutdown_system(ShutdownPhase::StopIngress, |_world| {
            panic!("shutdown failure")
        });
        app.world().resource::<AppControl>().unwrap().shutdown();

        app.run();

        assert_eq!(*calls.lock().unwrap(), [1, 2, 3, 4, 5]);
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "condition timed out");
            std::thread::yield_now();
        }
    }
}
