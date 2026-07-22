use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

const MAX_IDENTIFIER_LENGTH: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentifierError {
    kind: &'static str,
    value: String,
    reason: &'static str,
}

impl IdentifierError {
    fn new(kind: &'static str, value: &str, reason: &'static str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
            reason,
        }
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} `{}`: {}",
            self.kind, self.value, self.reason
        )
    }
}

impl std::error::Error for IdentifierError {}

fn validate_identifier(kind: &'static str, value: &str) -> Result<(), IdentifierError> {
    if value.is_empty() {
        return Err(IdentifierError::new(kind, value, "must not be empty"));
    }
    if value.len() > MAX_IDENTIFIER_LENGTH {
        return Err(IdentifierError::new(
            kind,
            value,
            "must not exceed 128 bytes",
        ));
    }
    if value == "." || value == ".." {
        return Err(IdentifierError::new(kind, value, "is reserved"));
    }
    if value.contains('/') || value.contains('\\') {
        return Err(IdentifierError::new(
            kind,
            value,
            "must not contain path separators",
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(IdentifierError::new(
            kind,
            value,
            "must not contain whitespace",
        ));
    }
    Ok(())
}

macro_rules! define_identifier {
    ($name:ident, $kind:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
                Self::try_from(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdentifierError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::try_from(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdentifierError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                validate_identifier($kind, value)?;
                Ok(Self(value.to_owned()))
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentifierError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                validate_identifier($kind, &value)?;
                Ok(Self(value))
            }
        }
    };
}

define_identifier!(WorkspaceId, "workspace ID");
define_identifier!(RequestId, "request ID");
define_identifier!(TaskId, "task ID");
define_identifier!(AgentId, "agent ID");
define_identifier!(ResourceId, "resource ID");
define_identifier!(ProjectName, "project name");
