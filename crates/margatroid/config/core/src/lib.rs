mod error;
pub mod types;

pub use error::ConfigError;
pub use types::{CidrBlock, FileToolConfig, IngressPolicy, IngressRequest, LogLevel};

use core_plugin::Resource;
use std::net::SocketAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MargatroidConfig {
    server_bind: SocketAddr,
    ingress: IngressPolicy,
    log_level: LogLevel,
    log_filter: Option<String>,
    tools: FileToolConfig,
}

impl MargatroidConfig {
    pub fn new(
        server_bind: SocketAddr,
        log_level: LogLevel,
        log_filter: Option<String>,
    ) -> Result<Self, ConfigError> {
        if log_filter
            .as_deref()
            .is_some_and(|filter| filter.trim().is_empty())
        {
            return Err(ConfigError::InvalidLogFilter);
        }
        Ok(Self {
            server_bind,
            ingress: IngressPolicy::Localhost,
            log_level,
            log_filter,
            tools: FileToolConfig::default(),
        })
    }

    pub fn with_ingress(mut self, ingress: IngressPolicy) -> Result<Self, ConfigError> {
        if self.server_bind.ip().is_loopback() && ingress.admits_non_loopback() {
            return Err(ConfigError::InvalidIngress);
        }
        if let IngressPolicy::Addresses(blocks) = &ingress
            && blocks
                .iter()
                .any(|block| block.covers_everything() || block.is_unspecified_host())
        {
            return Err(ConfigError::InvalidIngress);
        }
        self.ingress = ingress;
        Ok(self)
    }

    pub fn with_tools(mut self, tools: FileToolConfig) -> Result<Self, ConfigError> {
        tools.validate()?;
        self.tools = tools;
        Ok(self)
    }

    pub fn tools(&self) -> &FileToolConfig {
        &self.tools
    }

    pub fn server_bind(&self) -> SocketAddr {
        self.server_bind
    }

    pub fn server_ingress(&self) -> &IngressPolicy {
        &self.ingress
    }

    pub fn log_level(&self) -> LogLevel {
        self.log_level
    }

    pub fn log_filter(&self) -> Option<&str> {
        self.log_filter.as_deref()
    }
}

impl Resource for MargatroidConfig {}
