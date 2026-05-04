pub mod error;
pub mod traits;

pub mod openrouter;

pub use error::ProviderError;
pub use traits::{AiProvider, DynAiProvider};

// 方便外部直接构造 provider
pub use openrouter::OpenRouterProvider;
