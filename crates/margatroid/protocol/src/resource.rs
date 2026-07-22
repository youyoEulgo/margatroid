use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ResourceId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidDigest(String);

impl InvalidDigest {
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InvalidDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid content digest `{}`: expected sha256 followed by 64 lowercase hexadecimal characters",
            self.0
        )
    }
}

impl std::error::Error for InvalidDigest {}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ContentDigest(String);

impl ContentDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ContentDigest {
    type Err = InvalidDigest;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl TryFrom<&str> for ContentDigest {
    type Error = InvalidDigest;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl TryFrom<String> for ContentDigest {
    type Error = InvalidDigest;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if valid {
            Ok(Self(value))
        } else {
            Err(InvalidDigest(value))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Agent,
    Soul,
    Skill,
    Workflow,
    Provider,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ResourceReference {
    Installed { id: ResourceId },
    Bundled { digest: ContentDigest },
}
