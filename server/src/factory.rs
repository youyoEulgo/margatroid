use anyhow::Result;
use providers::DynAiProvider;
use std::sync::Arc;
use types::config::AiProvider;

pub fn build(config: &AiProvider) -> Result<Arc<dyn DynAiProvider>> {
    providers::build(config)
}
