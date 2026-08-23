use std::fmt;

use core_plugin::Entity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceIdError {
    Empty,
    InvalidType,
    InvalidScope,
    InvalidName,
    InvalidTag,
    InvalidFormat,
}

impl fmt::Display for ResourceIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "resource id cannot be empty",
            Self::InvalidType => "resource type is invalid",
            Self::InvalidScope => "resource scope is invalid",
            Self::InvalidName => "resource name is invalid",
            Self::InvalidTag => "resource tag is invalid",
            Self::InvalidFormat => "resource id format is invalid",
        })
    }
}

impl std::error::Error for ResourceIdError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceIdLookupError {
    PluginMissing,
    Missing {
        id: crate::ResourceId,
    },
    Duplicate {
        id: crate::ResourceId,
        entities: Vec<Entity>,
    },
}

impl fmt::Display for ResourceIdLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PluginMissing => formatter.write_str("ResourceIdPlugin is not installed"),
            Self::Missing { id } => write!(formatter, "resource `{id}` is missing"),
            Self::Duplicate { id, entities } => {
                write!(formatter, "resource `{id}` has {} entities", entities.len())
            }
        }
    }
}

impl std::error::Error for ResourceIdLookupError {}
