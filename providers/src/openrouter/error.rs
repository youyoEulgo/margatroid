use super::types::ApiError;
use crate::error::ProviderError;

#[derive(Debug)]
pub enum OpenRouterError {
    Http(reqwest::Error),
    Api(ApiError),
    ApiRaw {
        status: u16,
        body: String,
    },
    Deserialize {
        source: serde_json::Error,
        raw: String,
    },
    StreamChunk {
        source: serde_json::Error,
        raw: String,
    },
}

impl std::fmt::Display for OpenRouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Api(e) => write!(f, "API error {}: {}", e.code, e.message),
            Self::ApiRaw { status, body } => write!(f, "API error (HTTP {status}): {body}"),
            Self::Deserialize { source, raw } => {
                write!(f, "Deserialize error: {source}; raw: {raw}")
            }
            Self::StreamChunk { source, raw } => {
                write!(f, "Stream chunk error: {source}; chunk: {raw}")
            }
        }
    }
}

impl std::error::Error for OpenRouterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Deserialize { source, .. } | Self::StreamChunk { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for OpenRouterError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl From<OpenRouterError> for ProviderError {
    fn from(e: OpenRouterError) -> Self {
        match e {
            OpenRouterError::Http(e) => Self::Network(e.to_string()),
            OpenRouterError::Api(e) => Self::Api {
                code: e.code,
                message: e.message,
                metadata: e.metadata,
            },
            OpenRouterError::ApiRaw { status, body } => Self::ApiRaw { status, body },
            OpenRouterError::Deserialize { source, raw } => Self::Deserialize {
                message: source.to_string(),
                raw,
            },
            OpenRouterError::StreamChunk { source, raw } => Self::StreamChunk {
                message: source.to_string(),
                raw,
            },
        }
    }
}
