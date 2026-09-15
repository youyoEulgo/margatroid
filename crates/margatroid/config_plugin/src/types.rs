use std::collections::HashSet;
use std::net::SocketAddr;

use core_plugin::Resource;
use serde::Deserialize;

use crate::ConfigError;

pub(crate) const MAX_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WebSocketMessageTarget {
    Broadcast,
    Type(String),
    Name(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MargatroidConfig {
    server_bind: SocketAddr,
    log_level: LogLevel,
    log_filter: Option<String>,
    logs: Vec<WebSocketMessageTarget>,
    backend_state: Vec<WebSocketMessageTarget>,
    member_messages: Vec<WebSocketMessageTarget>,
    streaming_member_messages: Vec<WebSocketMessageTarget>,
}

impl MargatroidConfig {
    pub fn new(
        server_bind: SocketAddr,
        log_level: LogLevel,
        log_filter: Option<String>,
        logs: Vec<WebSocketMessageTarget>,
        backend_state: Vec<WebSocketMessageTarget>,
        member_messages: Vec<WebSocketMessageTarget>,
        streaming_member_messages: Vec<WebSocketMessageTarget>,
    ) -> Result<Self, ConfigError> {
        validate_targets("logs", &logs)?;
        validate_targets("backend_state", &backend_state)?;
        validate_targets("member_messages", &member_messages)?;
        validate_targets("streaming_member_messages", &streaming_member_messages)?;
        if log_filter
            .as_deref()
            .is_some_and(|filter| filter.trim().is_empty())
        {
            return Err(ConfigError::InvalidLogFilter);
        }
        Ok(Self {
            server_bind,
            log_level,
            log_filter,
            logs,
            backend_state,
            member_messages,
            streaming_member_messages,
        })
    }

    pub fn server_bind(&self) -> SocketAddr {
        self.server_bind
    }

    pub fn log_level(&self) -> LogLevel {
        self.log_level
    }

    pub fn log_filter(&self) -> Option<&str> {
        self.log_filter.as_deref()
    }

    pub fn logs(&self) -> &[WebSocketMessageTarget] {
        &self.logs
    }

    pub fn backend_state(&self) -> &[WebSocketMessageTarget] {
        &self.backend_state
    }

    pub fn member_messages(&self) -> &[WebSocketMessageTarget] {
        &self.member_messages
    }

    pub fn streaming_member_messages(&self) -> &[WebSocketMessageTarget] {
        &self.streaming_member_messages
    }
}

impl Resource for MargatroidConfig {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigDocument {
    pub(crate) server: ServerDocument,
    pub(crate) log: Option<LogDocument>,
    pub(crate) outbound: OutboundDocument,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServerDocument {
    pub(crate) bind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogDocument {
    pub(crate) level: Option<String>,
    pub(crate) filter: Option<String>,
}

impl TryFrom<ConfigDocument> for MargatroidConfig {
    type Error = ConfigError;

    fn try_from(document: ConfigDocument) -> Result<Self, Self::Error> {
        let server_bind = document
            .server
            .bind
            .parse()
            .map_err(|_| ConfigError::InvalidServerBind)?;
        let (log_level, log_filter) = match document.log {
            Some(log) => (
                match log.level {
                    Some(level) => decode_log_level(&level)?,
                    None => LogLevel::default(),
                },
                log.filter,
            ),
            None => (LogLevel::default(), None),
        };
        let outbound = document.outbound;
        Self::new(
            server_bind,
            log_level,
            log_filter,
            decode_targets("logs", outbound.logs)?,
            decode_targets("backend_state", outbound.backend_state)?,
            decode_targets("member_messages", outbound.member_messages)?,
            decode_targets(
                "streaming_member_messages",
                outbound.streaming_member_messages,
            )?,
        )
    }
}

fn decode_log_level(level: &str) -> Result<LogLevel, ConfigError> {
    match level {
        "off" => Ok(LogLevel::Off),
        "error" => Ok(LogLevel::Error),
        "warn" => Ok(LogLevel::Warn),
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        "trace" => Ok(LogLevel::Trace),
        _ => Err(ConfigError::InvalidLogLevel),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutboundDocument {
    pub(crate) logs: Vec<String>,
    pub(crate) backend_state: Vec<String>,
    pub(crate) member_messages: Vec<String>,
    pub(crate) streaming_member_messages: Vec<String>,
}

fn decode_targets(
    field: &'static str,
    targets: Vec<String>,
) -> Result<Vec<WebSocketMessageTarget>, ConfigError> {
    targets
        .into_iter()
        .map(|target| match target.as_str() {
            "broadcast" => Ok(WebSocketMessageTarget::Broadcast),
            _ => match target.split_once(':') {
                Some(("type", value)) if valid_value(value) => {
                    Ok(WebSocketMessageTarget::Type(value.into()))
                }
                Some(("name", value)) if valid_value(value) => {
                    Ok(WebSocketMessageTarget::Name(value.into()))
                }
                _ => Err(ConfigError::InvalidTarget(field)),
            },
        })
        .collect()
}

fn validate_targets(
    field: &'static str,
    targets: &[WebSocketMessageTarget],
) -> Result<(), ConfigError> {
    if targets.is_empty() {
        return Err(ConfigError::EmptyTargets(field));
    }
    let mut unique = HashSet::new();
    for target in targets {
        let valid = match target {
            WebSocketMessageTarget::Broadcast => true,
            WebSocketMessageTarget::Type(value) | WebSocketMessageTarget::Name(value) => {
                valid_value(value)
            }
        };
        if !valid {
            return Err(ConfigError::InvalidTarget(field));
        }
        if !unique.insert(target) {
            return Err(ConfigError::DuplicateTarget(field));
        }
    }
    Ok(())
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConfigPlugin;
    use core_plugin::App;

    #[test]
    fn loads_all_target_groups() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            include_str!("../../../../apps/daemon/config.example.toml"),
        )
        .unwrap();

        let plugin = ConfigPlugin::open(path).unwrap();
        assert_eq!(
            plugin.config.server_bind(),
            "127.0.0.1:3939".parse().unwrap()
        );
        assert_eq!(
            plugin.config.logs(),
            &[
                WebSocketMessageTarget::Type("cli".into()),
                WebSocketMessageTarget::Type("webui".into()),
            ]
        );
        assert_eq!(
            plugin.config.streaming_member_messages(),
            &[WebSocketMessageTarget::Type("webui".into())]
        );
    }

    #[test]
    fn rejects_unknown_target_prefixes() {
        assert_eq!(
            decode_targets("logs", vec!["client:cli".into()]).unwrap_err(),
            ConfigError::InvalidTarget("logs")
        );
    }

    #[test]
    fn rejects_invalid_server_bind() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let source = include_str!("../../../../apps/daemon/config.example.toml")
            .replace("127.0.0.1:3939", "not-an-address");
        std::fs::write(&path, source).unwrap();

        assert_eq!(
            ConfigPlugin::open(path).unwrap_err(),
            ConfigError::InvalidServerBind
        );
    }

    #[test]
    fn rejects_empty_target_groups() {
        let error = MargatroidConfig::new(
            "127.0.0.1:3939".parse().unwrap(),
            LogLevel::default(),
            None,
            Vec::new(),
            vec![WebSocketMessageTarget::Broadcast],
            vec![WebSocketMessageTarget::Broadcast],
            vec![WebSocketMessageTarget::Broadcast],
        )
        .unwrap_err();
        assert_eq!(error, ConfigError::EmptyTargets("logs"));
    }

    #[test]
    fn config_is_installed_as_a_resource() {
        let config = MargatroidConfig::new(
            "127.0.0.1:3939".parse().unwrap(),
            LogLevel::default(),
            None,
            vec![WebSocketMessageTarget::Broadcast],
            vec![WebSocketMessageTarget::Broadcast],
            vec![WebSocketMessageTarget::Broadcast],
            vec![WebSocketMessageTarget::Broadcast],
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugin(ConfigPlugin::new(config.clone()));
        assert_eq!(
            app.world().get_resource::<MargatroidConfig>(),
            Some(&config)
        );
    }

    #[test]
    fn defaults_to_info_level_without_a_log_section() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            concat!(
                "[server]\n",
                "bind = \"127.0.0.1:3939\"\n\n",
                "[outbound]\n",
                "logs = [\"broadcast\"]\n",
                "backend_state = [\"broadcast\"]\n",
                "member_messages = [\"broadcast\"]\n",
                "streaming_member_messages = [\"broadcast\"]\n",
            ),
        )
        .unwrap();

        let plugin = ConfigPlugin::open(path).unwrap();
        assert_eq!(plugin.config.log_level(), LogLevel::Info);
        assert_eq!(plugin.config.log_filter(), None);
    }

    #[test]
    fn loads_log_level_and_filter() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let source = include_str!("../../../../apps/daemon/config.example.toml").replace(
            "level = \"info\"",
            "level = \"debug\"\nfilter = \"info,mcl_plugin::system=debug\"",
        );
        std::fs::write(&path, source).unwrap();

        let plugin = ConfigPlugin::open(path).unwrap();
        assert_eq!(plugin.config.log_level(), LogLevel::Debug);
        assert_eq!(
            plugin.config.log_filter(),
            Some("info,mcl_plugin::system=debug")
        );
    }

    #[test]
    fn rejects_unknown_log_level() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let source = include_str!("../../../../apps/daemon/config.example.toml")
            .replace("level = \"info\"", "level = \"verbose\"");
        std::fs::write(&path, source).unwrap();

        assert_eq!(
            ConfigPlugin::open(path).unwrap_err(),
            ConfigError::InvalidLogLevel
        );
    }

    #[test]
    fn rejects_empty_log_filter() {
        let error = MargatroidConfig::new(
            "127.0.0.1:3939".parse().unwrap(),
            LogLevel::default(),
            Some("  ".into()),
            vec![WebSocketMessageTarget::Broadcast],
            vec![WebSocketMessageTarget::Broadcast],
            vec![WebSocketMessageTarget::Broadcast],
            vec![WebSocketMessageTarget::Broadcast],
        )
        .unwrap_err();
        assert_eq!(error, ConfigError::InvalidLogFilter);
    }
}
