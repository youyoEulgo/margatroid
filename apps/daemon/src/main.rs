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
            "--margatroid-tool-supervisor" | "--margatroid-tool-worker"
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
#[track_caller]
fn persist_notice(
    store: &storage_core::RuntimeLogStore,
    stage: &str,
    message: &str,
    details: serde_json::Value,
) {
    let location = std::panic::Location::caller();
    let record = storage_core::RuntimeLogInput {
        report_id: None,
        reporter: "backend".into(),
        target: "daemon".into(),
        agent: None,
        record: storage_core::AgentLogRecord {
            created_at_ms: None,
            level: storage_core::AgentLogLevel::Info,
            source: storage_core::AgentLogSource::Host,
            stage: stage.into(),
            message: message.into(),
            details_json: Some(details),
            context_json: Some(serde_json::json!({"process_id":std::process::id()})),
            origin: Some(format!("{}:{}", location.file(), location.line())),
            raw_error: None,
            traceback: None,
        },
    };
    if let Err(error) = store.append(record) {
        eprintln!("daemon notice could not be persisted: {error}; original: {message}");
    }
}
#[track_caller]
fn persist_failure(
    store: &storage_core::RuntimeLogStore,
    stage: &str,
    error: &(impl std::fmt::Debug + std::fmt::Display),
) {
    let location = std::panic::Location::caller();
    let record = storage_core::RuntimeLogInput {
        report_id: None,
        reporter: "backend".into(),
        target: "daemon".into(),
        agent: None,
        record: storage_core::AgentLogRecord {
            created_at_ms: None,
            level: storage_core::AgentLogLevel::Error,
            source: storage_core::AgentLogSource::Host,
            stage: stage.into(),
            message: error.to_string(),
            details_json: Some(serde_json::json!({"error":format!("{error:?}")})),
            context_json: Some(serde_json::json!({"process_id":std::process::id()})),
            origin: Some(format!("{}:{}", location.file(), location.line())),
            raw_error: Some(format!("{error:?}")),
            traceback: Some(std::backtrace::Backtrace::force_capture().to_string()),
        },
    };
    if let Err(failure) = store.append(record) {
        eprintln!("daemon failure could not be persisted: {failure}; original: {error}");
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
    let data_root = options.data_root.clone();
    let mut app = build_app(options)?;
    let logs = app
        .world()
        .get_resource::<storage_core::RuntimeLogStore>()
        .expect("persistent logs installed")
        .clone();
    persist_notice(
        &logs,
        "startup",
        "margatroid-daemon: starting",
        serde_json::json!({"data_directory":data_root}),
    );
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
        listen_for_shutdown(stopping.clone(), handle, logs.clone())?
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
            persist_notice(&logs,"startup","margatroid-daemon: started successfully",serde_json::json!({"listening":address.to_string(),"webui":format!("http://{browser_address}/"),"websocket":format!("ws://{browser_address}/ws")}));
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
                    persist_failure(&logs,"cleanup",&error);
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
    if let Err(error) = shutdown {
        persist_failure(&logs, "shutdown", &error);
        return Err(Box::new(error));
    }
    persist_notice(
        &logs,
        "shutdown",
        "margatroid-daemon: shutdown complete",
        serde_json::json!({}),
    );
    eprintln!("margatroid-daemon: shutdown complete");
    Ok(())
}

fn request_shutdown(
    stopping: &AtomicBool,
    handle: &RuntimeHandle,
    logs: &storage_core::RuntimeLogStore,
) {
    if !stopping.swap(true, Ordering::AcqRel) {
        persist_notice(logs,"shutdown","margatroid-daemon: shutdown requested; waiting for tasks and child processes to finish cleanup",serde_json::json!({}));
        eprintln!("margatroid-daemon: shutdown requested; waiting for tasks and child processes to finish cleanup");
        handle.wake();
    } else {
        persist_notice(
            logs,
            "shutdown",
            "margatroid-daemon: cleanup is still in progress",
            serde_json::json!({}),
        );
        eprintln!("margatroid-daemon: cleanup is still in progress");
    }
}

#[cfg(unix)]
fn listen_for_shutdown(
    stopping: Arc<AtomicBool>,
    handle: RuntimeHandle,
    logs: storage_core::RuntimeLogStore,
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
            request_shutdown(&stopping, &handle, &logs);
        }
    }))
}

#[cfg(not(unix))]
fn listen_for_shutdown(
    stopping: Arc<AtomicBool>,
    handle: RuntimeHandle,
    logs: storage_core::RuntimeLogStore,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    Ok(tokio::spawn(async move {
        loop {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("margatroid-daemon: signal listener failed: {error}");
                request_shutdown(&stopping, &handle, &logs);
                break;
            }
            request_shutdown(&stopping, &handle, &logs);
        }
    }))
}
