#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerPluginOptions {
    pub(crate) log_endpoint: Option<LogEndpointOptions>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LogEndpointOptions {
    bearer_token: String,
}

impl std::fmt::Debug for LogEndpointOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogEndpointOptions")
            .field("bearer_token", &"[REDACTED]")
            .finish()
    }
}

impl LogEndpointOptions {
    pub fn bearer_token(token: impl Into<String>) -> Self {
        let token = token.into();
        assert!(
            !token.is_empty(),
            "log endpoint bearer token cannot be empty"
        );
        Self {
            bearer_token: token,
        }
    }

    pub(crate) fn authorization_header(&self) -> String {
        format!("Bearer {}", self.bearer_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_bearer_token() {
        let options = LogEndpointOptions::bearer_token("top-secret");
        let debug = format!("{options:?}");
        assert!(!debug.contains("top-secret"));
        assert!(debug.contains("[REDACTED]"));
    }
}
