use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process;

use agent_image_loader_plugin::AgentImageLoaderPlugin;
use agent_plugin::AgentPlugin;
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimePlugin;
use config_plugin::ConfigPlugin;
use connection_plugin::ConnectionPlugin;
use core_plugin::App;
use dto_plugin::DtoPlugin;
use hook_plugin::HookPlugin;
use inference_plugin::InferencePlugin;
use log_plugin::LogPlugin;
use lua_plugin::LuaPlugin;
use lua_runtime_plugin::LuaRuntimePlugin;
use mcl_plugin::MclPlugin;
use memory_plugin::MemoryPlugin;
use resource_id_plugin::ResourceIdPlugin;
use server_plugin::{ServerOptions, ServerPlugin};
use shell_plugin::ShellPlugin;
use skill_plugin::SkillPlugin;
use tool_plugin::ToolPlugin;
use tracing::info;
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
    let agent_images_root = data_root.join("agent-images");
    let config_path = data_root.join("config.toml");
    let models_path = data_root.join("models.toml");
    if !models_path.is_file() {
        return Err(format!(
            "model route configuration is missing: {}",
            models_path.display()
        )
        .into());
    }
    let agent_images = AgentImageLoaderPlugin::open(&agent_images_root)
        .map_err(|error| format!("cannot open agent image root: {error}"))?;
    let workspace = WorkspacePlugin::open(&agent_images_root)
        .map_err(|error| format!("cannot open workspace agent image root: {error}"))?;
    let skill = SkillPlugin::open(data_root.join("skills"))
        .map_err(|error| format!("cannot open skill root: {error}"))?;
    let hook = HookPlugin::open(data_root.join("hooks"))
        .map_err(|error| format!("cannot open hook root: {error}"))?;
    let lua = LuaPlugin::open(data_root.join("tools"))
        .map_err(|error| format!("cannot open lua tool root: {error}"))?;
    let shell = ShellPlugin::open(data_root.join("shells"))
        .map_err(|error| format!("cannot open shell root: {error}"))?;
    let global_config = ConfigPlugin::open(&config_path)
        .map_err(|error| format!("cannot open global configuration: {error}"))?;
    let bind = global_config.config().server_bind();
    let mcl = MclPlugin::open(&data_root)
        .map_err(|error| format!("cannot open MCL resource root: {error}"))?;

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(LuaRuntimePlugin::default())
        .add_plugin(LogPlugin::default().with_stream(LOG_STREAM_CAPACITY))
        .add_plugin(ServerPlugin::with_options(ServerOptions::bind(bind)))
        .add_plugin(global_config)
        .add_plugin(ResourceIdPlugin)
        .add_plugin(agent_images)
        .add_plugin(InferencePlugin::default().with_config_path(models_path.clone()))
        .add_plugin(ToolPlugin::default())
        .add_plugin(skill)
        .add_plugin(hook)
        .add_plugin(lua)
        .add_plugin(shell)
        .add_plugin(MemoryPlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(mcl)
        .add_plugin(workspace)
        .add_plugin(DtoPlugin::default())
        .add_plugin(ConnectionPlugin::default());

    info!(address = %bind, data_root = %data_root.display(), config = %config_path.display(), models = %models_path.display(), "margatroid daemon starting");
    app.run();
    Ok(())
}

fn data_root() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(DATA_ROOT_NAME))
}
