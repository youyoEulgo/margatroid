use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolInput {
    pub description: String,
    pub input_schema: serde_json::Value,
    pub program: String,
    pub origin: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolReadLimits {
    pub manifest_bytes: usize,
    pub schema_bytes: usize,
    pub program_bytes: usize,
}

impl Default for ToolReadLimits {
    fn default() -> Self {
        Self {
            manifest_bytes: 1024 * 1024,
            schema_bytes: 1024 * 1024,
            program_bytes: 8 * 1024 * 1024,
        }
    }
}
