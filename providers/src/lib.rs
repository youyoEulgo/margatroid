pub mod error;
pub mod traits;

pub mod human;
pub mod openrouter;

pub use traits::AiProvider;
// DynAiProvider 定义在 types crate 中，这里重导出方便使用
pub use types::DynAiProvider;

// 方便外部直接构造 provider
pub use openrouter::OpenRouterProvider;

use anyhow::{Result, bail};
use std::sync::Arc;

/// 根据成员指定的 provider 名称，从配置中查找并构建 provider 实例
pub fn resolve(
    provider_name: &str,
    config: &types::config::ConfigAi,
) -> Result<Arc<dyn DynAiProvider>> {
    let provider_config = config
        .providers
        .iter()
        .find(|p| p.name == provider_name && p.enabled)
        .ok_or_else(|| anyhow::anyhow!("no enabled provider named '{}'", provider_name))?;
    build(provider_config)
}

/// 根据单个 provider 配置构建实例
pub fn build(config: &types::config::AiProvider) -> Result<Arc<dyn DynAiProvider>> {
    match config.provider_type.as_str() {
        "openrouter" => {
            let mut client = OpenRouterProvider::new(&config.api_key);
            if !config.base_url.is_empty() {
                client = client.with_base_url(&config.base_url);
            }
            Ok(Arc::new(client))
        }
        "human" => Ok(Arc::new(human::HumanProvider::new(config.base_url.clone()))),
        other => bail!("unsupported provider type: {}", other),
    }
}
