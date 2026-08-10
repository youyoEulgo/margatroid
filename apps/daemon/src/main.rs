use std::error::Error;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process;

use agent_image_loader_plugin::AgentImageLoaderPlugin;
use agent_plugin::AgentPlugin;
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimePlugin;
use config_plugin::ConfigPlugin;
use connection_plugin::ConnectionPlugin;
use core_plugin::App;
use dto_plugin::DtoPlugin;
use inference_plugin::InferencePlugin;
use log_plugin::LogPlugin;
use memory_plugin::MemoryPlugin;
use server_plugin::{ServerOptions, ServerPlugin};
use skill_plugin::SkillPlugin;
use tool_plugin::ToolPlugin;
use tracing::info;
use workflow_plugin::WorkflowPlugin;
use workspace_plugin::WorkspacePlugin;

const DEFAULT_BIND: &str = "127.0.0.1:3939";
const DEFAULT_DATA_ROOT: &str = ".margatroid";
const LOG_STREAM_CAPACITY: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
struct DaemonConfig {
    bind: SocketAddr,
    data_root: PathBuf,
}

fn main() {
    match parse_args(std::env::args().skip(1)) {
        Ok(config) => {
            if let Err(error) = run(config) {
                eprintln!("margatroid-daemon: {error}");
                process::exit(1);
            }
        }
        Err(error) if error == usage() => println!("{}", usage()),
        Err(error) => {
            eprintln!("margatroid-daemon: {error}");
            eprintln!();
            eprintln!("{}", usage());
            process::exit(2);
        }
    }
}

fn run(config: DaemonConfig) -> Result<(), Box<dyn Error + Send + Sync>> {
    fs::create_dir_all(&config.data_root)?;
    let data_root = absolute_path(&config.data_root)?;
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
    let skills = SkillPlugin::open(data_root.join("skills"))
        .map_err(|error| format!("cannot open skill root: {error}"))?;
    let workflows = WorkflowPlugin::open(data_root.join("workflows"))
        .map_err(|error| format!("cannot open workflow root: {error}"))?;
    let global_config = ConfigPlugin::open(&config_path)
        .map_err(|error| format!("cannot open global configuration: {error}"))?;

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(LogPlugin::default().with_stream(LOG_STREAM_CAPACITY))
        .add_plugin(ServerPlugin::with_options(ServerOptions::bind(config.bind)))
        .add_plugin(global_config)
        .add_plugin(agent_images)
        .add_plugin(InferencePlugin::default().with_config_path(models_path.clone()))
        .add_plugin(ToolPlugin::default())
        .add_plugin(skills)
        .add_plugin(workflows)
        .add_plugin(MemoryPlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(workspace)
        .add_plugin(DtoPlugin::default())
        .add_plugin(ConnectionPlugin::default());

    info!(address = %config.bind, data_root = %data_root.display(), config = %config_path.display(), models = %models_path.display(), "margatroid daemon starting");
    app.run();
    Ok(())
}

fn parse_args<I>(arguments: I) -> Result<DaemonConfig, String>
where
    I: IntoIterator<Item = String>,
{
    let mut arguments = arguments.into_iter();
    let mut bind = DEFAULT_BIND.parse().expect("default bind address is valid");
    let mut data_root = default_data_root();
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Err(usage().to_owned());
        }
        if argument == "--bind" {
            bind = parse_bind(arguments.next().ok_or("--bind requires an address")?)?;
            continue;
        }
        if let Some(value) = argument.strip_prefix("--bind=") {
            bind = parse_bind(value.to_owned())?;
            continue;
        }
        if argument == "--data-root" {
            data_root = PathBuf::from(arguments.next().ok_or("--data-root requires a directory")?);
            continue;
        }
        if let Some(value) = argument.strip_prefix("--data-root=") {
            if value.is_empty() {
                return Err("--data-root requires a directory".into());
            }
            data_root = PathBuf::from(value);
            continue;
        }
        return Err(format!("unknown option '{argument}'"));
    }
    Ok(DaemonConfig { bind, data_root })
}

fn parse_bind(value: String) -> Result<SocketAddr, String> {
    value
        .parse()
        .map_err(|error| format!("invalid bind address '{value}': {error}"))
}

fn default_data_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_DATA_ROOT)
}

fn absolute_path(path: &Path) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn usage() -> &'static str {
    "Usage: margatroid-daemon [--bind HOST:PORT] [--data-root DIRECTORY]\n\nStart the Margatroid backend WebSocket server."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_local_server_and_home_data_root() {
        let config = parse_args(std::iter::empty()).unwrap();
        assert_eq!(config.bind, DEFAULT_BIND.parse().unwrap());
        assert!(config.data_root.ends_with(DEFAULT_DATA_ROOT));
    }

    #[test]
    fn accepts_bind_and_data_root_options() {
        let config = parse_args([
            "--bind".into(),
            "0.0.0.0:4000".into(),
            "--data-root=/tmp/margatroid-test".into(),
        ])
        .unwrap();
        assert_eq!(config.bind, "0.0.0.0:4000".parse().unwrap());
        assert_eq!(config.data_root, PathBuf::from("/tmp/margatroid-test"));
    }
}
