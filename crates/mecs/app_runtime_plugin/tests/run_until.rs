use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use app_runtime_plugin::{AppRunExt, RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Event, World};

struct Notice;
impl Event for Notice {}

fn stop_from_thread(app: &mut App, handle: RuntimeHandle) {
    let stopped = Arc::new(AtomicBool::new(false));
    let stop_signal = Arc::clone(&stopped);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        ready_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        stop_signal.store(true, Ordering::Release);
        handle.wake();
    });
    let start = Instant::now();
    app.run_until(|| {
        let _ = ready_tx.try_send(());
        stopped.load(Ordering::Acquire)
    });
    worker.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn external_stop_wakes_event_driven_runtime_and_allows_event_ticks() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default());
    let handle = app.world().get_resource::<RuntimeHandle>().unwrap().clone();
    let sender = app.world().event_sender();
    let startup = Arc::new(AtomicUsize::new(0));
    let startup_calls = Arc::clone(&startup);
    app.add_system(RuntimePlugin::STARTUP, move |_world: &mut World| {
        startup_calls.fetch_add(1, Ordering::Relaxed);
    });

    stop_from_thread(&mut app, handle.clone());

    assert_eq!(startup.load(Ordering::Relaxed), 1);
    handle.wake();
    sender.send_event(Notice);
    app.tick();
    assert_eq!(app.world().event_reader::<Notice>().len(), 1);
    app.run_until(|| true);
    stop_from_thread(&mut app, handle);
    assert_eq!(startup.load(Ordering::Relaxed), 1);
}

#[test]
fn already_stopped_preserves_startup_and_runtime_for_both_modes() {
    for plugin in [RuntimePlugin::default(), RuntimePlugin::fixed(60)] {
        let mut app = App::new();
        app.add_plugin(plugin);
        let startup = Arc::new(AtomicUsize::new(0));
        let startup_calls = Arc::clone(&startup);
        app.add_system(RuntimePlugin::STARTUP, move |_world: &mut World| {
            startup_calls.fetch_add(1, Ordering::Relaxed);
        });

        app.run_until(|| true);

        assert_eq!(startup.load(Ordering::Relaxed), 0);
        app.world().get_resource::<RuntimeHandle>().unwrap().wake();
        app.run_until(|| true);
        app.tick();
        assert_eq!(startup.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn stop_wakes_closed_gate_without_startup_or_changing_blocker_count() {
    for plugin in [RuntimePlugin::default(), RuntimePlugin::fixed(60)] {
        let mut app = App::new();
        app.add_plugin(plugin);
        let handle = app.world().get_resource::<RuntimeHandle>().unwrap().clone();
        let startup = Arc::new(AtomicUsize::new(0));
        let startup_calls = Arc::clone(&startup);
        app.add_system(RuntimePlugin::STARTUP, move |_world: &mut World| {
            startup_calls.fetch_add(1, Ordering::Relaxed);
        });
        handle.close_gate();
        handle.close_gate();

        stop_from_thread(&mut app, handle.clone());
        assert_eq!(startup.load(Ordering::Relaxed), 0);
        handle.open_gate();
        stop_from_thread(&mut app, handle.clone());
        assert_eq!(startup.load(Ordering::Relaxed), 0);
        handle.open_gate();
        stop_from_thread(&mut app, handle);
        assert_eq!(startup.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn fixed_frame_stops_after_original_frame_wait() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::fixed(60));
    let handle = app.world().get_resource::<RuntimeHandle>().unwrap().clone();
    stop_from_thread(&mut app, handle);
    app.world().send_event(Notice);
    app.tick();
    assert_eq!(app.world().event_reader::<Notice>().len(), 1);
}

#[test]
fn event_driven_run_until_keeps_delayed_event_fast_forwarding() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default());
    let stopped = Arc::new(AtomicBool::new(false));
    let stop_signal = Arc::clone(&stopped);
    let frames = Arc::new(AtomicUsize::new(0));
    let frame_count = Arc::clone(&frames);
    app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
        frame_count.fetch_add(1, Ordering::Relaxed);
        if !world.event_reader::<Notice>().is_empty() {
            stop_signal.store(true, Ordering::Release);
        }
    });
    app.world().send_event_after(Notice, 10_000);

    app.run_until(|| stopped.load(Ordering::Acquire));

    assert_eq!(frames.load(Ordering::Relaxed), 2);
    app.world().get_resource::<RuntimeHandle>().unwrap().wake();
}

#[test]
fn original_run_entry_point_remains_available() {
    let _run: fn(&mut App) = <App as AppRunExt>::run;
}
