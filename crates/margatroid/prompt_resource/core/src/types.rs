#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptInput {
    pub role: PromptRole,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptReadLimits {
    pub manifest_bytes: usize,
    pub content_bytes: usize,
}

impl Default for PromptReadLimits {
    fn default() -> Self {
        Self {
            manifest_bytes: 1024 * 1024,
            content_bytes: 8 * 1024 * 1024,
        }
    }
}
