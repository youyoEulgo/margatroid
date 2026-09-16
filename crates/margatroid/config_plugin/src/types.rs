use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, SocketAddr};

use core_plugin::Resource;
use serde::Deserialize;
use server_plugin::HandshakeGuard;

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
    server_ingress: IngressPolicy,
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
            server_ingress: IngressPolicy::localhost(),
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

    pub fn with_ingress(mut self, ingress: IngressPolicy) -> Self {
        self.server_ingress = ingress;
        self
    }

    pub fn server_ingress(&self) -> &IngressPolicy {
        &self.server_ingress
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
    pub(crate) allow: Option<toml::Value>,
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
        let ingress = decode_ingress(document.server.allow.as_ref(), server_bind)?;
        let outbound = document.outbound;
        Ok(Self::new(
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
        )?
        .with_ingress(ingress))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CidrBlock {
    network: IpAddr,
    prefix: u8,
}

impl CidrBlock {
    pub fn parse(value: &str) -> Option<Self> {
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (value, None),
        };
        let network = address.parse::<IpAddr>().ok()?;
        let bits = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix = match prefix {
            Some(prefix) => prefix.parse::<u8>().ok()?,
            None => bits,
        };
        if prefix > bits {
            return None;
        }
        Some(Self { network, prefix })
    }

    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let mask = u32::MAX
                    .checked_shl(u32::from(32 - self.prefix))
                    .unwrap_or(0);
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let mask = u128::MAX
                    .checked_shl(u32::from(128 - self.prefix))
                    .unwrap_or(0);
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }

    pub fn network(&self) -> IpAddr {
        self.network
    }

    pub fn prefix(&self) -> u8 {
        self.prefix
    }

    pub fn covers_everything(&self) -> bool {
        self.prefix == 0
    }

    pub fn is_unspecified_host(&self) -> bool {
        if !self.network.is_unspecified() {
            return false;
        }
        match self.network {
            IpAddr::V4(_) => self.prefix == 32,
            IpAddr::V6(_) => self.prefix == 128,
        }
    }

    fn is_loopback_only(&self) -> bool {
        if !self.network.is_loopback() {
            return false;
        }
        match self.network {
            IpAddr::V4(_) => self.prefix >= 8,
            IpAddr::V6(_) => self.prefix >= 128,
        }
    }
}

impl fmt::Display for CidrBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngressPolicy {
    Localhost,
    Any,
    Addresses(Vec<CidrBlock>),
}

impl IngressPolicy {
    pub fn localhost() -> Self {
        Self::Localhost
    }

    pub fn any() -> Self {
        Self::Any
    }

    pub fn addresses(blocks: Vec<CidrBlock>) -> Self {
        Self::Addresses(blocks)
    }

    pub fn allows_peer(&self, peer: IpAddr) -> bool {
        match self {
            Self::Localhost => peer.is_loopback(),
            Self::Any => true,
            Self::Addresses(blocks) => blocks.iter().any(|block| block.contains(peer)),
        }
    }

    pub fn admits_non_loopback(&self) -> bool {
        match self {
            Self::Localhost => false,
            Self::Any => true,
            Self::Addresses(blocks) => blocks.iter().any(|block| !block.is_loopback_only()),
        }
    }

    pub fn allows_origin(&self, origin: Option<&str>, host: Option<&str>) -> bool {
        self.evaluate_origin(origin, host).is_ok()
    }

    pub fn evaluate(
        &self,
        peer: SocketAddr,
        origin: Option<&str>,
        host: Option<&str>,
    ) -> Result<(), String> {
        if !self.allows_peer(peer.ip()) {
            return Err(format!("peer {peer} is not allowed by {self}"));
        }
        self.evaluate_origin(origin, host)
    }

    fn evaluate_origin(&self, origin: Option<&str>, host: Option<&str>) -> Result<(), String> {
        let Some(origin) = origin else {
            return Ok(());
        };
        if origin == "null" {
            return Err("origin null is not a reachable origin".to_owned());
        }
        let Some(authority) = authority_of(origin) else {
            return Err(format!("origin {origin} has no authority"));
        };
        let Some(host) = host else {
            return Err(format!(
                "origin {authority} cannot be checked without a host"
            ));
        };
        if !authority.eq_ignore_ascii_case(host) {
            return Err(format!("origin {authority} does not match host {host}"));
        }
        Ok(())
    }
}

impl HandshakeGuard for IngressPolicy {
    fn guard(
        &self,
        peer: SocketAddr,
        origin: Option<&str>,
        host: Option<&str>,
    ) -> Result<(), String> {
        self.evaluate(peer, origin, host)
    }
}

impl fmt::Display for IngressPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Localhost => formatter.write_str("localhost"),
            Self::Any => formatter.write_str("all"),
            Self::Addresses(blocks) => {
                let rendered = blocks
                    .iter()
                    .map(CidrBlock::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                write!(formatter, "{rendered}")
            }
        }
    }
}

pub(crate) fn decode_ingress(
    allow: Option<&toml::Value>,
    bind: SocketAddr,
) -> Result<IngressPolicy, ConfigError> {
    let policy = match allow {
        None => IngressPolicy::localhost(),
        Some(toml::Value::String(scalar)) => match scalar.as_str() {
            "localhost" | "127.0.0.1" => IngressPolicy::localhost(),
            "all" | "0.0.0.0" => IngressPolicy::any(),
            _ => return Err(ConfigError::InvalidIngressAllow),
        },
        Some(toml::Value::Array(entries)) => {
            let mut blocks = Vec::with_capacity(entries.len());
            for (index, entry) in entries.iter().enumerate() {
                let Some(entry) = entry.as_str() else {
                    return Err(ConfigError::InvalidIngressEntry(index));
                };
                let Some(block) = CidrBlock::parse(entry) else {
                    return Err(ConfigError::InvalidIngressEntry(index));
                };
                if block.covers_everything() {
                    return Err(ConfigError::IngressEntryCoversEverything(index));
                }
                if block.is_unspecified_host() {
                    return Err(ConfigError::InvalidIngressEntry(index));
                }
                blocks.push(block);
            }
            IngressPolicy::addresses(blocks)
        }
        Some(_) => return Err(ConfigError::InvalidIngressAllow),
    };
    if bind.ip().is_loopback() && policy.admits_non_loopback() {
        return Err(ConfigError::IngressWiderThanBind);
    }
    Ok(policy)
}

fn authority_of(origin: &str) -> Option<&str> {
    let origin = match origin.split_once("://") {
        Some((_, rest)) => rest,
        None => origin,
    };
    let origin = origin.rsplit_once('@').map_or(origin, |(_, rest)| rest);
    let authority = origin
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() {
        None
    } else {
        Some(authority)
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

    use std::net::{Ipv4Addr, Ipv6Addr};

    fn loopback_bind() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 3939))
    }

    fn public_bind() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 3939))
    }

    fn peer(address: &str) -> SocketAddr {
        SocketAddr::new(address.parse().unwrap(), 50000)
    }

    fn entry(value: &str) -> toml::Value {
        toml::Value::String(value.to_owned())
    }

    #[test]
    fn missing_allow_defaults_to_localhost() {
        let policy = decode_ingress(None, loopback_bind()).unwrap();
        assert_eq!(policy, IngressPolicy::localhost());
        assert!(policy.allows_peer(Ipv4Addr::LOCALHOST.into()));
        assert!(!policy.allows_peer(Ipv4Addr::new(10, 0, 0, 5).into()));
    }

    #[test]
    fn scalar_allows_the_loopback_and_wildcard_spellings() {
        for scalar in ["localhost", "127.0.0.1"] {
            assert_eq!(
                decode_ingress(Some(&entry(scalar)), public_bind()).unwrap(),
                IngressPolicy::localhost()
            );
        }
        for scalar in ["all", "0.0.0.0"] {
            assert_eq!(
                decode_ingress(Some(&entry(scalar)), public_bind()).unwrap(),
                IngressPolicy::any()
            );
        }
    }

    #[test]
    fn unknown_scalars_are_rejected() {
        assert_eq!(
            decode_ingress(Some(&entry("example.com")), public_bind()),
            Err(ConfigError::InvalidIngressAllow)
        );
    }

    #[test]
    fn arrays_decode_into_cidr_blocks() {
        let allow = toml::Value::Array(vec![entry("10.0.0.0/8"), entry("192.168.1.7")]);
        let policy = decode_ingress(Some(&allow), public_bind()).unwrap();
        assert!(policy.allows_peer(Ipv4Addr::new(10, 4, 5, 6).into()));
        assert!(policy.allows_peer(Ipv4Addr::new(192, 168, 1, 7).into()));
        assert!(!policy.allows_peer(Ipv4Addr::new(192, 168, 1, 8).into()));
        assert_eq!(policy.to_string(), "10.0.0.0/8,192.168.1.7/32");
    }

    #[test]
    fn invalid_entries_report_their_index() {
        let allow = toml::Value::Array(vec![entry("10.0.0.0/8"), entry("10.0.0.0/33")]);
        assert_eq!(
            decode_ingress(Some(&allow), public_bind()),
            Err(ConfigError::InvalidIngressEntry(1))
        );

        let mixed = toml::Value::Array(vec![toml::Value::Integer(7)]);
        assert_eq!(
            decode_ingress(Some(&mixed), public_bind()),
            Err(ConfigError::InvalidIngressEntry(0))
        );
    }

    #[test]
    fn entries_that_cover_everything_are_rejected() {
        let allow = toml::Value::Array(vec![entry("0.0.0.0/0")]);
        assert_eq!(
            decode_ingress(Some(&allow), public_bind()),
            Err(ConfigError::IngressEntryCoversEverything(0))
        );
        let bare = toml::Value::Array(vec![entry("0.0.0.0")]);
        assert_eq!(
            decode_ingress(Some(&bare), public_bind()),
            Err(ConfigError::InvalidIngressEntry(0))
        );
    }

    #[test]
    fn loopback_lists_are_accepted_but_wider_ones_are_not() {
        let loopback = toml::Value::Array(vec![entry("127.0.0.1/32"), entry("::1/128")]);
        let policy = decode_ingress(Some(&loopback), loopback_bind()).unwrap();
        assert!(policy.allows_peer(Ipv6Addr::LOCALHOST.into()));
        assert!(!policy.admits_non_loopback());

        let wider = toml::Value::Array(vec![entry("10.0.0.0/8")]);
        assert_eq!(
            decode_ingress(Some(&wider), loopback_bind()),
            Err(ConfigError::IngressWiderThanBind)
        );
        assert!(decode_ingress(Some(&wider), public_bind()).is_ok());
    }

    #[test]
    fn peers_are_matched_within_their_own_address_family() {
        let block = CidrBlock::parse("127.0.0.0/8").unwrap();
        assert!(block.contains(Ipv4Addr::LOCALHOST.into()));
        assert!(!block.contains("::ffff:127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(CidrBlock::parse("::1")
            .unwrap()
            .contains(Ipv6Addr::LOCALHOST.into()));
    }

    #[test]
    fn origins_must_match_the_host_authority() {
        let policy = IngressPolicy::any();
        assert!(policy.allows_origin(None, Some("127.0.0.1:3939")));
        assert!(policy.allows_origin(Some("http://127.0.0.1:3939"), Some("127.0.0.1:3939")));
        assert!(policy.allows_origin(Some("https://127.0.0.1:3939"), Some("127.0.0.1:3939")));
        assert!(policy.allows_origin(
            Some("http://user@127.0.0.1:3939/path?q=1"),
            Some("127.0.0.1:3939")
        ));
        assert!(!policy.allows_origin(Some("http://127.0.0.1:3000"), Some("127.0.0.1:3939")));
        assert!(!policy.allows_origin(Some("http://evil.example"), Some("127.0.0.1:3939")));
        assert!(!policy.allows_origin(Some("null"), Some("127.0.0.1:3939")));
        assert!(!policy.allows_origin(Some("http://127.0.0.1:3939"), None));
    }

    #[test]
    fn evaluation_reports_the_rejected_peer_or_origin() {
        let policy = IngressPolicy::localhost();
        let rejected = policy.evaluate(peer("10.0.0.5"), None, None).unwrap_err();
        assert!(rejected.contains("peer 10.0.0.5:50000"));
        assert!(rejected.contains("localhost"));

        let rejected = policy
            .evaluate(
                peer("127.0.0.1"),
                Some("http://127.0.0.1:3000"),
                Some("127.0.0.1:3939"),
            )
            .unwrap_err();
        assert!(rejected.contains("does not match host"));
        assert!(policy.evaluate(peer("127.0.0.1"), None, None).is_ok());
    }
}
