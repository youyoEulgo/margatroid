use core_plugin::{App, Plugin};

use crate::AppControl;

#[derive(Clone, Copy, Debug, Default)]
pub struct AppRuntimePlugin;

impl Plugin for AppRuntimePlugin {
    fn build(&self, app: &mut App) {
        if app.world().resource::<AppControl>().is_none() {
            app.add_resource(AppControl::new());
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

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "condition timed out");
            std::thread::yield_now();
        }
    }
}
