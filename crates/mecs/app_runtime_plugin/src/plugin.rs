use core_plugin::{App, Plugin, World};

use crate::shutdown::ShutdownSystems;
use crate::AppControl;

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
    fn on_shutdown(&mut self, system: impl FnMut(&mut World) + Send + 'static) -> &mut Self;
    fn after_shutdown(&mut self, system: impl FnMut(&mut World) + Send + 'static) -> &mut Self;
}

impl AppShutdownExt for App {
    fn on_shutdown(&mut self, system: impl FnMut(&mut World) + Send + 'static) -> &mut Self {
        self.world_mut()
            .resource::<ShutdownSystems>()
            .unwrap_or_else(|| panic!("AppRuntimePlugin must be installed before shutdown systems"))
            .add(system);
        self
    }

    fn after_shutdown(&mut self, system: impl FnMut(&mut World) + Send + 'static) -> &mut Self {
        self.world_mut()
            .resource::<ShutdownSystems>()
            .unwrap_or_else(|| panic!("AppRuntimePlugin must be installed before shutdown systems"))
            .add_finalizer(system);
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
    fn shutdown_systems_run_in_reverse_registration_order_and_isolate_panics() {
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        for value in 1..=5 {
            let calls = calls.clone();
            app.on_shutdown(move |_world| calls.lock().unwrap().push(value));
        }
        app.on_shutdown(|_world| panic!("shutdown failure"));
        let final_calls = calls.clone();
        app.after_shutdown(move |_world| final_calls.lock().unwrap().push(6));
        app.world().resource::<AppControl>().unwrap().shutdown();

        app.run();

        assert_eq!(*calls.lock().unwrap(), [5, 4, 3, 2, 1, 6]);
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "condition timed out");
            std::thread::yield_now();
        }
    }
}
