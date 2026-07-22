use serde::{Deserialize, Serialize};

pub const API_VERSION: &str = "v1";
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(u32);

impl SchemaVersion {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn current() -> Self {
        Self(CURRENT_SCHEMA_VERSION)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    pub const fn is_supported(self) -> bool {
        self.0 == CURRENT_SCHEMA_VERSION
    }
}

impl Default for SchemaVersion {
    fn default() -> Self {
        Self::current()
    }
}
