use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use types::DynAiProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProviderRegistryError {
    EmptyName,
}

impl fmt::Display for LlmProviderRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmProviderRegistryError::EmptyName => write!(f, "provider name cannot be empty"),
        }
    }
}

impl std::error::Error for LlmProviderRegistryError {}

#[derive(Clone)]
pub struct LlmProviderRegistry {
    providers: Arc<RwLock<HashMap<String, Arc<dyn DynAiProvider>>>>,
}

impl LlmProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(
        &self,
        name: impl Into<String>,
        provider: Arc<dyn DynAiProvider>,
    ) -> Result<(), LlmProviderRegistryError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(LlmProviderRegistryError::EmptyName);
        }
        self.providers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name, provider);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn DynAiProvider>> {
        self.providers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .providers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }
}

impl Default for LlmProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
