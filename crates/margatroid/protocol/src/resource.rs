use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ResourceId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidAgentImageReference(String);

impl InvalidAgentImageReference {
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InvalidAgentImageReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid agent image reference `{}`: expected scope/name[:tag] or scope/name@sha256:<digest>",
            self.0
        )
    }
}

impl std::error::Error for InvalidAgentImageReference {}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct AgentImageReference(String);

impl AgentImageReference {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidAgentImageReference> {
        Self::try_from(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentImageReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for AgentImageReference {
    type Error = InvalidAgentImageReference;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > 255 {
            return Err(InvalidAgentImageReference(value));
        }
        let (name, version) = if let Some((name, digest)) = value.split_once('@') {
            let valid_digest = digest.strip_prefix("sha256:").is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
            (name, valid_digest)
        } else if let Some((name, tag)) = value.rsplit_once(':') {
            (name, valid_reference_segment(tag, 128))
        } else {
            (value.as_str(), true)
        };

        let mut segments = name.split('/');
        let scope = segments.next().unwrap_or_default();
        let image = segments.next().unwrap_or_default();
        let valid_name = valid_reference_segment(scope, 128)
            && valid_reference_segment(image, 128)
            && segments.next().is_none()
            && !name.contains([':', '@']);

        if valid_name && version {
            Ok(Self(value))
        } else {
            Err(InvalidAgentImageReference(value))
        }
    }
}

fn valid_reference_segment(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

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
