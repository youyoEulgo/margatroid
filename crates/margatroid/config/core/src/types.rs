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
