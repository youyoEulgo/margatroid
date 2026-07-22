use std::sync::{Arc, Barrier};

use core_plugin::App;
use log_plugin::{LogPlugin, LogStream, LogStreamOptions};

#[test]
fn concurrent_installers_share_only_requested_managed_stream() {
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let mut app = App::new();
            barrier.wait();
            app.add_plugins(LogPlugin::default().with_stream(LogStreamOptions::with_capacity(8)));
            app.world().resource::<LogStream>().is_some()
        }));
    }

    assert!(threads.into_iter().all(|thread| thread.join().unwrap()));

    let mut app = App::new();
    app.add_plugins(LogPlugin::default());
    assert!(app.world().resource::<LogStream>().is_none());
}
