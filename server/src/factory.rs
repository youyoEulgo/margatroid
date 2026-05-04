use anyhow::{Result, bail};
use providers::{DynAiProvider, OpenRouterProvider};
use std::sync::Arc;
use types::config::AiProvider;

pub fn build(config: &AiProvider) -> Result<Arc<dyn DynAiProvider>> {
    match config.provider_type.as_str() {
        "openrouter" => {
            let mut client = OpenRouterProvider::new(&config.api_key);
            if !config.base_url.is_empty() {
                client = client.with_base_url(&config.base_url);
            }
            Ok(Arc::new(client))
        }
        other => bail!("unsupported provider type: {}", other),
    }
}
