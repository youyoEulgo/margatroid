use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process;

use agent_driver_loader_plugin::AgentDriverLoaderPlugin;
use agent_plugin::AgentPlugin;
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimePlugin;
use client_plugin::ClientPlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use config_plugin::{ConfigPlugin, LogLevel as ConfigLogLevel};
use core_plugin::App;
use dto_plugin::DtoPlugin;
use inference_plugin::InferencePlugin;
use log_plugin::{LogLevel, LogPlugin};
use lua_runtime_plugin::LuaRuntimePlugin;
use mcl_plugin::MclPlugin;
use memory_plugin::MemoryPlugin;
use prompt_resource_plugin::PromptResourcePlugin;
use resource_id_plugin::ResourcePlugin;
use sandbox_resource_plugin::SandboxResourcePlugin;
use server_plugin::{AppServerExt, ServerOptions, ServerPlugin};
use tool_plugin::ToolPlugin;
use tool_resource_plugin::ToolResourcePlugin;
use tracing::info;
use ui_plugin::UiPlugin;
use workspace_plugin::WorkspacePlugin;

const DATA_ROOT_NAME: &str = ".margatroid";
const LOG_STREAM_CAPACITY: usize = 256;

fn main() {
    if let Err(error) = run() {
        eprintln!("margatroid-daemon: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let data_root = data_root()?;
    fs::create_dir_all(&data_root)?;
    let agent_drivers_root = data_root.join("agent-drivers");
    let config_path = data_root.join("config.toml");
    let models_path = data_root.join("models.toml");
    if !models_path.is_file() {
        return Err(format!(
            "model route configuration is missing: {}",
            models_path.display()
        )
        .into());
    }
    let agent_drivers = AgentDriverLoaderPlugin::open(&agent_drivers_root)
        .map_err(|error| format!("cannot open agent driver root: {error}"))?;
    let workspace = WorkspacePlugin::default();
    let global_config = ConfigPlugin::open(&config_path)
        .map_err(|error| format!("cannot open global configuration: {error}"))?;
    let bind = global_config.config().server_bind();
    let ingress = global_config.config().server_ingress().clone();
    let mcl = MclPlugin::default();
    let mut log_plugin = LogPlugin::default()
        .with_stream(LOG_STREAM_CAPACITY)
        .with_level(log_level(global_config.config().log_level()));
    if let Some(filter) = global_config.config().log_filter() {
        log_plugin = log_plugin.with_filter(filter);
    }

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(LuaRuntimePlugin::default())
        .add_plugin(log_plugin)
        .add_plugin(ServerPlugin::with_options(
            ServerOptions::bind(bind).with_handshake_guard(ingress.clone()),
        ))
        .add_plugin(global_config)
        .add_plugin(ResourcePlugin)
        .add_plugin(ToolResourcePlugin::open(data_root.clone())?)
        .add_plugin(PromptResourcePlugin::open(data_root.clone())?)
        .add_plugin(SandboxResourcePlugin::open(data_root.clone())?)
        .add_plugin(agent_drivers)
        .add_plugin(
            InferencePlugin::default()
                .with_config_path(models_path.clone())
                .with_schedule(RuntimePlugin::UPDATE),
        )
        .add_plugin(ToolPlugin::default())
        .add_plugin(MemoryPlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(mcl)
        .add_plugin(workspace)
        .add_plugin(DtoPlugin::default())
        .add_plugin(ClientPlugin::default())
        .add_plugin(UiPlugin::default());

    app.add_http_routes(UiPlugin::router());

    info!(address = %bind, allow = %ingress, data_root = %data_root.display(), config = %config_path.display(), models = %models_path.display(), "margatroid daemon starting");
    app.run();
    Ok(())
}

fn data_root() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(DATA_ROOT_NAME))
}

fn log_level(level: ConfigLogLevel) -> LogLevel {
    match level {
        ConfigLogLevel::Off => LogLevel::Off,
        ConfigLogLevel::Error => LogLevel::Error,
        ConfigLogLevel::Warn => LogLevel::Warn,
        ConfigLogLevel::Info => LogLevel::Info,
        ConfigLogLevel::Debug => LogLevel::Debug,
        ConfigLogLevel::Trace => LogLevel::Trace,
    }
}
