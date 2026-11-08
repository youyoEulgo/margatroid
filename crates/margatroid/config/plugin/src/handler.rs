use config_core::{
    CidrBlock, ConfigError, FileToolConfig, IngressPolicy, LogLevel, MargatroidConfig,
    OutboundTarget,
};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

pub(crate) fn read_config(path: &Path) -> Result<MargatroidConfig, ConfigError> {
    let failed = || ConfigError::ReadFailed(path.to_path_buf());
    let metadata = std::fs::metadata(path).map_err(|_| failed())?;
    if !metadata.is_file() {
        return Err(failed());
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge);
    }
    let file = File::open(path).map_err(|_| failed())?;
    if !file.metadata().map_err(|_| failed())?.is_file() {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failed())?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge);
    }
    let source = std::str::from_utf8(&bytes).map_err(|_| ConfigError::DecodeFailed)?;
    let document: ConfigDocument = toml::from_str(source).map_err(|_| ConfigError::DecodeFailed)?;
    let bind = document
        .server
        .bind
        .parse()
        .map_err(|_| ConfigError::InvalidBind)?;
    let ingress = decode_ingress(document.server.allow.as_ref())?;
    let (log_level, log_filter) = match document.log {
        None => (LogLevel::default(), None),
        Some(log) => (
            match log.level.as_deref() {
                None | Some("info") => LogLevel::Info,
                Some("off") => LogLevel::Off,
                Some("error") => LogLevel::Error,
                Some("warn") => LogLevel::Warn,
                Some("debug") => LogLevel::Debug,
                Some("trace") => LogLevel::Trace,
                Some(_) => return Err(ConfigError::InvalidLogLevel),
            },
            log.filter,
        ),
    };
    MargatroidConfig::new(
        bind,
        log_level,
        log_filter,
        decode_targets(document.outbound.logs)?,
        decode_targets(document.outbound.backend_state)?,
    )?
    .with_ingress(ingress)?
    .with_tools(document.tools)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigDocument {
    server: ServerDocument,
    log: Option<LogDocument>,
    outbound: OutboundDocument,
    #[serde(default)]
    tools: FileToolConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerDocument {
    bind: String,
    allow: Option<toml::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogDocument {
    level: Option<String>,
    filter: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboundDocument {
    logs: Vec<String>,
    backend_state: Vec<String>,
}

fn decode_ingress(allow: Option<&toml::Value>) -> Result<IngressPolicy, ConfigError> {
    match allow {
        None => Ok(IngressPolicy::Localhost),
        Some(toml::Value::String(scalar)) => match scalar.as_str() {
            "localhost" | "127.0.0.1" => Ok(IngressPolicy::Localhost),
            "all" | "0.0.0.0" => Ok(IngressPolicy::Any),
            _ => Err(ConfigError::InvalidIngress),
        },
        Some(toml::Value::Array(entries)) => {
            let blocks = entries
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .and_then(CidrBlock::parse)
                        .ok_or(ConfigError::InvalidIngress)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(IngressPolicy::Addresses(blocks))
        }
        Some(_) => Err(ConfigError::InvalidIngress),
    }
}

fn decode_targets(targets: Vec<String>) -> Result<Vec<OutboundTarget>, ConfigError> {
    targets
        .into_iter()
        .map(|target| match target.as_str() {
            "broadcast" => Ok(OutboundTarget::Broadcast),
            _ => match target.split_once(':') {
                Some(("type", value)) => Ok(OutboundTarget::Type(value.into())),
                Some(("name", value)) => Ok(OutboundTarget::Name(value.into())),
                _ => Err(ConfigError::InvalidTarget),
            },
        })
        .collect()
}
