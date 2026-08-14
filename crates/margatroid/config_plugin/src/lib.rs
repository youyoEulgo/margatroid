use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

use core_plugin::{App, Plugin, Resource};
use serde::Deserialize;

const MAX_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WebSocketMessageTarget {
    Broadcast,
    Type(String),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MargatroidConfig {
    server_bind: SocketAddr,
    logs: Vec<WebSocketMessageTarget>,
    backend_state: Vec<WebSocketMessageTarget>,
    member_messages: Vec<WebSocketMessageTarget>,
    streaming_member_messages: Vec<WebSocketMessageTarget>,
}

impl MargatroidConfig {
    pub fn new(
        server_bind: SocketAddr,
        logs: Vec<WebSocketMessageTarget>,
        backend_state: Vec<WebSocketMessageTarget>,
        member_messages: Vec<WebSocketMessageTarget>,
        streaming_member_messages: Vec<WebSocketMessageTarget>,
    ) -> Result<Self, ConfigError> {
        validate_targets("logs", &logs)?;
        validate_targets("backend_state", &backend_state)?;
        validate_targets("member_messages", &member_messages)?;
        validate_targets("streaming_member_messages", &streaming_member_messages)?;
        Ok(Self {
            server_bind,
            logs,
            backend_state,
            member_messages,
            streaming_member_messages,
        })
    }

    pub fn server_bind(&self) -> SocketAddr {
        self.server_bind
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

#[derive(Clone, Debug)]
pub struct ConfigPlugin {
    config: MargatroidConfig,
}

impl ConfigPlugin {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        let source = fs::read_to_string(&path).map_err(|_| ConfigError::ReadFailed(path))?;
        if source.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooLarge);
        }
        let document =
            toml::from_str::<ConfigDocument>(&source).map_err(|_| ConfigError::DecodeFailed)?;
        Ok(Self {
            config: document.try_into()?,
        })
    }

    pub fn new(config: MargatroidConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &MargatroidConfig {
        &self.config
    }
}

impl Plugin for ConfigPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<MargatroidConfig>() {
            panic!("ConfigPlugin is already installed");
        }
        app.world_mut().insert_resource(self.config);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    ReadFailed(PathBuf),
    TooLarge,
    DecodeFailed,
    InvalidServerBind,
    EmptyTargets(&'static str),
    InvalidTarget(&'static str),
    DuplicateTarget(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFailed(path) => {
                write!(
                    formatter,
                    "configuration could not be read: {}",
                    path.display()
                )
            }
            Self::TooLarge => formatter.write_str("configuration exceeds the size limit"),
            Self::DecodeFailed => formatter.write_str("configuration could not be decoded"),
            Self::InvalidServerBind => {
                formatter.write_str("configuration field `server.bind` is not a socket address")
            }
            Self::EmptyTargets(field) => {
                write!(formatter, "configuration field `{field}` has no targets")
            }
            Self::InvalidTarget(field) => {
                write!(
                    formatter,
                    "configuration field `{field}` contains an invalid target"
                )
            }
            Self::DuplicateTarget(field) => {
                write!(
                    formatter,
                    "configuration field `{field}` contains a duplicate target"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigDocument {
    server: ServerDocument,
    outbound: OutboundDocument,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerDocument {
    bind: String,
}

impl TryFrom<ConfigDocument> for MargatroidConfig {
    type Error = ConfigError;

    fn try_from(document: ConfigDocument) -> Result<Self, Self::Error> {
        let server_bind = document
            .server
            .bind
            .parse()
            .map_err(|_| ConfigError::InvalidServerBind)?;
        let outbound = document.outbound;
        Self::new(
            server_bind,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboundDocument {
    logs: Vec<String>,
    backend_state: Vec<String>,
    member_messages: Vec<String>,
    streaming_member_messages: Vec<String>,
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

    #[test]
    fn loads_all_target_groups() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
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
        fs::write(&path, source).unwrap();

        assert_eq!(
            ConfigPlugin::open(path).unwrap_err(),
            ConfigError::InvalidServerBind
        );
    }

    #[test]
    fn rejects_empty_target_groups() {
        let error = MargatroidConfig::new(
            "127.0.0.1:3939".parse().unwrap(),
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
}
