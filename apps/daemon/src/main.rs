use app_runtime_plugin::AppRunExt;
use compose::{build_app, ApplicationOptions};
use std::path::PathBuf;
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
fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let options = ApplicationOptions {
        data_root: PathBuf::from(home).join(".margatroid"),
        runner_path: std::env::current_exe()?,
    };
    let mut app = build_app(options)?;
    app.run();
    Ok(())
}
