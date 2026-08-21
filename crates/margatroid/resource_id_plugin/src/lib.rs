use std::fmt;
use std::str::FromStr;

use core_plugin::{Component, Entity, Plugin, Resource, World};
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId {
    resource_type: String,
    scope: String,
    name: String,
    tag: String,
}

impl ResourceId {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ResourceIdError> {
        value.as_ref().parse()
    }

    pub fn new(
        resource_type: impl Into<String>,
        scope: impl Into<String>,
        name: impl Into<String>,
        tag: Option<impl Into<String>>,
    ) -> Result<Self, ResourceIdError> {
        let resource_type = resource_type.into();
        let scope = scope.into();
        let name = name.into();
        let tag = tag.map(Into::into).unwrap_or_else(|| "latest".into());
        validate_type(&resource_type)?;
        validate_part(&scope, ResourceIdError::InvalidScope)?;
        validate_part(&name, ResourceIdError::InvalidName)?;
        validate_tag(&tag)?;
        Ok(Self {
            resource_type,
            scope,
            name,
            tag,
        })
    }

    pub fn resource_type(&self) -> &str {
        &self.resource_type
    }
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn tag(&self) -> &str {
        &self.tag
    }
}

impl FromStr for ResourceId {
    type Err = ResourceIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(ResourceIdError::Empty);
        }
        let (resource_type, remainder) = value
            .split_once(':')
            .ok_or(ResourceIdError::InvalidFormat)?;
        let (scope, name_and_tag) = remainder
            .split_once('/')
            .ok_or(ResourceIdError::InvalidFormat)?;
        if name_and_tag.contains('/') {
            return Err(ResourceIdError::InvalidFormat);
        }
        let (name, tag) = match name_and_tag.split_once(':') {
            Some((name, tag)) if !tag.contains(':') => (name, Some(tag.to_owned())),
            Some(_) => return Err(ResourceIdError::InvalidFormat),
            None => (name_and_tag, None),
        };
        Self::new(resource_type, scope, name, tag)
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}/{}:{}",
            self.resource_type, self.scope, self.name, self.tag
        )
    }
}

impl Serialize for ResourceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ResourceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl Component for ResourceId {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceIdLookupError {
    PluginMissing,
    Missing {
        id: ResourceId,
    },
    Duplicate {
        id: ResourceId,
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

pub struct ResourceIdPlugin;
pub struct ResourceIdPluginInstalled;
impl Resource for ResourceIdPluginInstalled {}

impl Plugin for ResourceIdPlugin {
    fn build(self, app: &mut core_plugin::App) {
        if app.world().contains_resource::<ResourceIdPluginInstalled>() {
            panic!("ResourceIdPlugin is already installed");
        }
        app.world_mut().insert_resource(ResourceIdPluginInstalled);
    }
}

pub trait WorldResourceIdExt {
    fn entity_by_resource_id(&self, id: &ResourceId) -> Result<Entity, ResourceIdLookupError>;
}

impl WorldResourceIdExt for World {
    fn entity_by_resource_id(&self, id: &ResourceId) -> Result<Entity, ResourceIdLookupError> {
        if !self.contains_resource::<ResourceIdPluginInstalled>() {
            return Err(ResourceIdLookupError::PluginMissing);
        }
        let mut entities = self
            .query_with::<ResourceId>()
            .result()
            .into_iter()
            .filter(|entity| {
                self.get_component::<ResourceId>(*entity)
                    .is_some_and(|value| value == id)
            })
            .collect::<Vec<_>>();
        entities.sort_by_key(|entity| entity.index());
        match entities.as_slice() {
            [] => Err(ResourceIdLookupError::Missing { id: id.clone() }),
            [entity] => Ok(*entity),
            _ => Err(ResourceIdLookupError::Duplicate {
                id: id.clone(),
                entities,
            }),
        }
    }
}

fn validate_type(value: &str) -> Result<(), ResourceIdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
    {
        Err(ResourceIdError::InvalidType)
    } else {
        Ok(())
    }
}

fn validate_part(value: &str, error: ResourceIdError) -> Result<(), ResourceIdError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value
            .bytes()
            .any(|c| c.is_ascii_control() || matches!(c, b'/' | b'\\' | b':'))
    {
        Err(error)
    } else {
        Ok(())
    }
}

fn validate_tag(value: &str) -> Result<(), ResourceIdError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(ResourceIdError::InvalidTag);
    };
    if !first.is_ascii_alphanumeric()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        Err(ResourceIdError::InvalidTag)
    } else {
        Ok(())
    }
}
