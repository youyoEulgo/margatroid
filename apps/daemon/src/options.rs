use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:3939";

pub(crate) struct DaemonOptions {
    pub(crate) bind_address: SocketAddr,
    pub(crate) data_dir: PathBuf,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) log_stream_bearer_token: Option<String>,
}

impl std::fmt::Debug for DaemonOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DaemonOptions")
            .field("bind_address", &self.bind_address)
            .field("data_dir", &self.data_dir)
            .field("config_path", &self.config_path)
            .field(
                "log_stream_bearer_token",
                &self.log_stream_bearer_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

pub(crate) enum OptionsOutcome {
    Run(DaemonOptions),
    Help,
}

#[derive(Default)]
struct CliOptions {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    config: Option<PathBuf>,
    help: bool,
}

#[derive(Default)]
struct EnvironmentOptions {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    config: Option<PathBuf>,
    log_stream_bearer_token: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    daemon: DaemonSection,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DaemonSection {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    log_stream_bearer_token: Option<String>,
}

impl DaemonOptions {
    pub(crate) fn load(args: impl IntoIterator<Item = OsString>) -> Result<OptionsOutcome> {
        let cli = CliOptions::parse(args)?;
        if cli.help {
            return Ok(OptionsOutcome::Help);
        }
        let environment = EnvironmentOptions::read();
        let default_data_dir = paths::margatroid_root()
            .ok_or_else(|| anyhow!("cannot determine the current user's home directory"))?;
        resolve(cli, environment, default_data_dir).map(OptionsOutcome::Run)
    }
}

impl CliOptions {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(argument) = args.next() {
            let argument = argument
                .to_str()
                .ok_or_else(|| anyhow!("daemon option is not valid UTF-8"))?;
            match argument {
                "-h" | "--help" => parsed.help = true,
                "--bind" => parsed.bind = Some(next_utf8(&mut args, "--bind")?),
                "--data-dir" => parsed.data_dir = Some(next_path(&mut args, "--data-dir")?),
                "--config" => parsed.config = Some(next_path(&mut args, "--config")?),
                _ => bail!("unknown daemon option `{argument}`"),
            }
        }
        Ok(parsed)
    }
}

impl EnvironmentOptions {
    fn read() -> Self {
        Self {
            bind: non_empty_env("MARGATROID_BIND"),
            data_dir: std::env::var_os("MARGATROID_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            config: std::env::var_os("MARGATROID_CONFIG")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            log_stream_bearer_token: non_empty_env("MARGATROID_LOG_TOKEN"),
        }
    }
}

fn resolve(
    cli: CliOptions,
    environment: EnvironmentOptions,
    default_data_dir: PathBuf,
) -> Result<DaemonOptions> {
    let bootstrap_data_dir = cli
        .data_dir
        .as_ref()
        .or(environment.data_dir.as_ref())
        .unwrap_or(&default_data_dir);
    let explicit_config = cli.config.is_some() || environment.config.is_some();
    let config_path = cli
        .config
        .clone()
        .or_else(|| environment.config.clone())
        .unwrap_or_else(|| {
            paths::DaemonPaths::new(bootstrap_data_dir)
                .config()
                .to_path_buf()
        });
    let file = read_config(&config_path, explicit_config)?;
    let file_data_dir = file
        .daemon
        .data_dir
        .map(|path| resolve_config_path(&config_path, path));

    let data_dir = cli
        .data_dir
        .or(environment.data_dir)
        .or(file_data_dir)
        .unwrap_or(default_data_dir);
    let bind = cli
        .bind
        .or(environment.bind)
        .or(file.daemon.bind)
        .unwrap_or_else(|| DEFAULT_BIND_ADDRESS.into());
    let bind_address = bind
        .parse()
        .with_context(|| format!("invalid daemon bind address `{bind}`"))?;
    let log_stream_bearer_token = environment
        .log_stream_bearer_token
        .or(file.daemon.log_stream_bearer_token)
        .map(validate_token)
        .transpose()?;

    Ok(DaemonOptions {
        bind_address,
        data_dir,
        config_path: config_path.exists().then_some(config_path),
        log_stream_bearer_token,
    })
}

fn read_config(path: &Path, required: bool) -> Result<ConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let config: ConfigFile = toml::from_str(&content)
                .map_err(|_| anyhow!("cannot parse daemon config {}", path.display()))?;
            if config.daemon.log_stream_bearer_token.is_some() {
                require_private_config(path)?;
            }
            Ok(config)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
            Ok(ConfigFile::default())
        }
        Err(error) => {
            Err(error).with_context(|| format!("cannot read daemon config {}", path.display()))
        }
    }
}

#[cfg(unix)]
fn require_private_config(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        bail!(
            "daemon config {} contains a bearer token and must not be accessible by group or others",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_private_config(_path: &Path) -> Result<()> {
    Ok(())
}

fn resolve_config_path(config_path: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn validate_token(token: String) -> Result<String> {
    if token.is_empty() {
        bail!("log stream bearer token cannot be empty");
    }
    Ok(token)
}

fn next_utf8(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("{option} requires a value"))?
        .into_string()
        .map_err(|_| anyhow!("{option} value is not valid UTF-8"))
}

fn next_path(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<PathBuf> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("{option} requires a value"))
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_is_cli_then_environment_then_file_then_default() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("daemon.toml");
        std::fs::write(
            &config,
            "[daemon]\nbind = '127.0.0.1:4001'\ndata_dir = 'from-file'\n",
        )
        .unwrap();
        let cli = CliOptions {
            bind: Some("127.0.0.1:4003".into()),
            config: Some(config.clone()),
            ..CliOptions::default()
        };
        let environment = EnvironmentOptions {
            bind: Some("127.0.0.1:4002".into()),
            data_dir: Some(directory.path().join("from-env")),
            ..EnvironmentOptions::default()
        };

        let options = resolve(cli, environment, directory.path().join("default")).unwrap();

        assert_eq!(options.bind_address, "127.0.0.1:4003".parse().unwrap());
        assert_eq!(options.data_dir, directory.path().join("from-env"));
        assert_eq!(options.config_path.as_deref(), Some(config.as_path()));
    }

    #[test]
    fn file_paths_are_relative_to_config_directory() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("daemon.toml");
        std::fs::write(&config, "[daemon]\ndata_dir = 'state'\n").unwrap();
        let cli = CliOptions {
            config: Some(config),
            ..CliOptions::default()
        };

        let options =
            resolve(cli, EnvironmentOptions::default(), PathBuf::from("default")).unwrap();

        assert_eq!(options.data_dir, directory.path().join("state"));
    }

    #[test]
    fn explicit_missing_config_is_an_error() {
        let cli = CliOptions {
            config: Some(PathBuf::from("definitely-missing.toml")),
            ..CliOptions::default()
        };
        assert!(resolve(cli, EnvironmentOptions::default(), PathBuf::from("default")).is_err());
    }

    #[test]
    fn default_bind_address_uses_product_port() {
        let options = resolve(
            CliOptions::default(),
            EnvironmentOptions::default(),
            PathBuf::from("definitely-missing-root"),
        )
        .unwrap();
        assert_eq!(options.bind_address, "127.0.0.1:3939".parse().unwrap());
    }

    #[test]
    fn parse_errors_do_not_expose_config_contents() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("daemon.toml");
        std::fs::write(&config, "[daemon]\nlog_stream_bearer_token = 'top-secret\n").unwrap();

        let error = match read_config(&config, true) {
            Ok(_) => panic!("malformed config should fail"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("cannot parse daemon config"));
        assert!(!error.contains("top-secret"));
    }

    #[test]
    fn unknown_config_fields_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("daemon.toml");
        std::fs::write(&config, "[daemon]\nbing = '127.0.0.1:4000'\n").unwrap();

        assert!(read_config(&config, true).is_err());
    }

    #[test]
    fn debug_redacts_log_stream_token() {
        let options = DaemonOptions {
            bind_address: "127.0.0.1:3939".parse().unwrap(),
            data_dir: PathBuf::from("state"),
            config_path: None,
            log_stream_bearer_token: Some("top-secret".into()),
        };

        let debug = format!("{options:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("top-secret"));
    }
}
