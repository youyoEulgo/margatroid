use std::fmt;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OutboundTarget {
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
        let bits = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix)
                if !prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                prefix.parse::<u8>().ok()?
            }
            Some(_) => return None,
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
        self.network.is_unspecified()
            && self.prefix == if self.network.is_ipv4() { 32 } else { 128 }
    }

    fn is_loopback_only(&self) -> bool {
        self.network.is_loopback() && self.prefix >= if self.network.is_ipv4() { 8 } else { 128 }
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

    pub fn evaluate(&self, request: &IngressRequest) -> Result<(), String> {
        if !self.allows_peer(request.peer.ip()) {
            return Err(format!("peer {} is not allowed by {self}", request.peer));
        }
        self.evaluate_origin(request.origin.as_deref(), request.host.as_deref())
    }

    fn evaluate_origin(&self, origin: Option<&str>, host: Option<&str>) -> Result<(), String> {
        let Some(origin) = origin else {
            return Ok(());
        };
        let (scheme, authority) = origin
            .split_once("://")
            .ok_or_else(|| "origin has no HTTP scheme".to_owned())?;
        let default_port = if scheme.eq_ignore_ascii_case("http") {
            80
        } else if scheme.eq_ignore_ascii_case("https") {
            443
        } else {
            return Err("origin has an invalid HTTP scheme".to_owned());
        };
        let (origin_name, origin_port) = parse_authority(authority)
            .ok_or_else(|| "origin has an invalid authority".to_owned())?;
        let host = host.ok_or_else(|| "origin cannot be checked without a host".to_owned())?;
        let (host_name, host_port) =
            parse_authority(host).ok_or_else(|| "host has an invalid authority".to_owned())?;
        if origin_name != host_name
            || origin_port.unwrap_or(default_port) != host_port.unwrap_or(default_port)
        {
            return Err(format!("origin {authority} does not match host {host}"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngressRequest {
    pub peer: SocketAddr,
    pub origin: Option<String>,
    pub host: Option<String>,
}

impl fmt::Display for IngressPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Localhost => formatter.write_str("localhost"),
            Self::Any => formatter.write_str("all"),
            Self::Addresses(blocks) => {
                for (index, block) in blocks.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(",")?;
                    }
                    write!(formatter, "{block}")?;
                }
                Ok(())
            }
        }
    }
}

fn parse_authority(value: &str) -> Option<(String, Option<u16>)> {
    if value.is_empty()
        || value.chars().any(|character| {
            character.is_whitespace()
                || character.is_control()
                || matches!(character, '/' | '\\' | '?' | '#' | '@' | '%')
        })
    {
        return None;
    }
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let (address, tail) = rest.split_once(']')?;
        let address = address.parse::<Ipv6Addr>().ok()?;
        let port = if tail.is_empty() {
            None
        } else {
            Some(tail.strip_prefix(':')?)
        };
        (address.to_string(), port)
    } else {
        let (host, port) = match value.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (value, None),
        };
        let hostname = host.strip_suffix('.').unwrap_or(host);
        if hostname.is_empty()
            || !hostname.split('.').all(|label| {
                !label.is_empty()
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    && !label.starts_with('-')
                    && !label.ends_with('-')
            })
        {
            return None;
        }
        (host.to_ascii_lowercase(), port)
    };
    let port = match port {
        Some(port) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => {
            Some(port.parse().ok()?)
        }
        Some(_) => return None,
        None => None,
    };
    Some((host, port))
}

/// Optional tool budgets; omitted values use the tool runtime's defaults at composition.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileToolConfig {
    #[serde(
        deserialize_with = "capacity::deserialize_option",
        serialize_with = "capacity::serialize_option"
    )]
    pub total_task_memory_limit: Option<usize>,
    #[serde(
        deserialize_with = "capacity::deserialize_option",
        serialize_with = "capacity::serialize_option"
    )]
    pub task_memory_limit: Option<usize>,
    #[serde(
        deserialize_with = "capacity::deserialize_option",
        serialize_with = "capacity::serialize_option"
    )]
    pub task_output_buffer_window: Option<usize>,
}

impl FileToolConfig {
    pub fn validate(&self) -> Result<(), crate::ConfigError> {
        for (field, value) in [
            (
                "tools.total_task_memory_limit",
                self.total_task_memory_limit,
            ),
            ("tools.task_memory_limit", self.task_memory_limit),
            (
                "tools.task_output_buffer_window",
                self.task_output_buffer_window,
            ),
        ] {
            if value == Some(0) {
                return Err(crate::ConfigError::InvalidToolLimit {
                    field,
                    reason: "must be greater than zero".into(),
                });
            }
        }
        Ok(())
    }
}

mod capacity {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn deserialize_option<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        value
            .map(|value| parse(&value).map_err(D::Error::custom))
            .transpose()
    }

    pub fn serialize_option<S>(value: &Option<usize>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.serialize_some(&format(*value)),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn parse(value: &str) -> Result<usize, String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("must be a positive whole number with unit B, K, M, G or T".into());
        }
        let split = value
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(value.len());
        let digits = &value[..split];
        let unit = &value[split..];
        if digits.is_empty() || !unit_valid(unit) {
            return Err("must be a positive whole number with unit B, K, M, G or T".into());
        }
        let number: usize = digits
            .parse()
            .map_err(|_| "numeric value overflows usize".to_string())?;
        if number == 0 {
            return Err("must be greater than zero".into());
        }
        let multiplier = match unit.to_ascii_lowercase().as_str() {
            "" | "b" => 1,
            "k" | "kb" | "kib" => 1024,
            "m" | "mb" | "mib" => 1024usize.pow(2),
            "g" | "gb" | "gib" => 1024usize.pow(3),
            "t" | "tb" | "tib" => 1024usize.pow(4),
            _ => unreachable!(),
        };
        number
            .checked_mul(multiplier)
            .ok_or_else(|| "capacity overflows usize".into())
    }

    fn unit_valid(unit: &str) -> bool {
        matches!(
            unit.to_ascii_lowercase().as_str(),
            "" | "b"
                | "k"
                | "kb"
                | "kib"
                | "m"
                | "mb"
                | "mib"
                | "g"
                | "gb"
                | "gib"
                | "t"
                | "tb"
                | "tib"
        )
    }

    fn format(value: usize) -> String {
        for (suffix, multiplier) in [
            ("T", 1024usize.pow(4)),
            ("G", 1024usize.pow(3)),
            ("M", 1024usize.pow(2)),
            ("K", 1024usize),
        ] {
            if value >= multiplier && value.is_multiple_of(multiplier) {
                return format!("{}{}", value / multiplier, suffix);
            }
        }
        format!("{value}B")
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::capacity::parse;

    #[test]
    fn parses_binary_units_case_insensitively() {
        assert_eq!(parse("2G"), Ok(2 * 1024usize.pow(3)));
        assert_eq!(parse("128m"), Ok(128 * 1024usize.pow(2)));
        assert_eq!(parse("4KiB"), Ok(4096));
        assert_eq!(parse("17"), Ok(17));
    }

    #[test]
    fn rejects_invalid_capacity_values() {
        for value in ["0", "-1G", "1.5G", "1X", "", " G"] {
            assert!(parse(value).is_err(), "{value}");
        }
    }
}
