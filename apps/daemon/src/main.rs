use app_runtime_plugin::{AppRunExt, RuntimeHandle};
use compose::{build_app, ApplicationOptions};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
fn main() {
    if std::env::args().nth(1).is_some_and(|arg| {
        matches!(
            arg.as_str(),
            "--margatroid-tool-supervisor"
                | "--margatroid-tool-worker"
                | "--margatroid-tool-memory-entry"
        )
    }) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tool runtime initialization failed");
        std::process::exit(runtime.block_on(tool_plugin::runner_main()));
    }
    if let Err(error) = run() {
        eprintln!("margatroid-daemon: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let options = ApplicationOptions {
        data_root: PathBuf::from(home).join(".margatroid"),
        runner_path: std::env::current_exe()?,
    };
    eprintln!(
        "margatroid-daemon: starting (data directory: {})",
        options.data_root.display()
    );
    let mut app = build_app(options)?;
    let server = app
        .world()
        .get_resource::<axum_plugin::AxumHandle>()
        .expect("Axum service installed")
        .clone();
    let reported = std::cell::Cell::new(false);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    let stopping = Arc::new(AtomicBool::new(false));
    let handle = app
        .world()
        .get_resource::<RuntimeHandle>()
        .expect("application runtime installed")
        .clone();
    let listener = {
        let _entered = runtime.enter();
        listen_for_shutdown(stopping.clone(), handle)?
    };
    app.run_until(|| {
        if stopping.load(Ordering::Acquire) || server.is_stopped() {
            return true;
        }
        if let Some(address) = server.local_address().filter(|_| !reported.get()) {
            let browser_address = match address.ip() {
                std::net::IpAddr::V4(ip) if ip.is_unspecified() => {
                    std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, address.port()))
                }
                std::net::IpAddr::V6(ip) if ip.is_unspecified() => {
                    std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, address.port()))
                }
                _ => address,
            };
            eprintln!("margatroid-daemon: started successfully (listening on {address})");
            eprintln!("Webui:  http://{browser_address}/");
            eprintln!("WebSocket: ws://{browser_address}/ws");
            reported.set(true);
        }
        false
    });
    let shutdown = runtime.block_on(async {
        let result = loop {
            match compose::shutdown_app(&mut app).await {
                Ok(()) => break Ok(()),
                // The installed Axum service reports its final error after domain drain.
                Err(error) if error.domain == "axum" => break Err(error),
                Err(error) => {
                    eprintln!("margatroid-daemon: cleanup is not complete: {error}; continuing to supervise");
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
        };
        listener.abort();
        let _ = listener.await;
        result
    });
    drop(app);
    shutdown?;
    eprintln!("margatroid-daemon: shutdown complete");
    Ok(())
}

fn request_shutdown(stopping: &AtomicBool, handle: &RuntimeHandle) {
    if !stopping.swap(true, Ordering::AcqRel) {
        eprintln!("margatroid-daemon: shutdown requested; waiting for tasks and child processes to finish cleanup");
        handle.wake();
    } else {
        eprintln!("margatroid-daemon: cleanup is still in progress");
    }
}

#[cfg(unix)]
fn listen_for_shutdown(
    stopping: Arc<AtomicBool>,
    handle: RuntimeHandle,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(tokio::spawn(async move {
        loop {
            tokio::select! {
                value = interrupt.recv() => if value.is_none() { break; },
                value = terminate.recv() => if value.is_none() { break; },
            }
            request_shutdown(&stopping, &handle);
        }
    }))
}

#[cfg(not(unix))]
fn listen_for_shutdown(
    stopping: Arc<AtomicBool>,
    handle: RuntimeHandle,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    Ok(tokio::spawn(async move {
        loop {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("margatroid-daemon: signal listener failed: {error}");
                request_shutdown(&stopping, &handle);
                break;
            }
            request_shutdown(&stopping, &handle);
        }
    }))
}
