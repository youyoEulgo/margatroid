use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ResourceIDError;

/// Validated address value, serialized as `type:scope/name:tag`.
/// Missing tags normalize to `latest`; this value is not an ECS component.
///
/// ```compile_fail
/// use core_plugin::Component;
/// use resource_id_plugin::ResourceID;
/// fn require_component<T: Component>() {}
/// require_component::<ResourceID>();
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceID {
    resource_type: String,
    scope: String,
    name: String,
    tag: String,
}

impl ResourceID {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ResourceIDError> {
        value.as_ref().parse()
    }

    pub fn new(
        resource_type: impl Into<String>,
        scope: impl Into<String>,
        name: impl Into<String>,
        tag: Option<String>,
    ) -> Result<Self, ResourceIDError> {
        let resource_type = resource_type.into();
        let scope = scope.into();
        let name = name.into();
        let tag = tag.unwrap_or_else(|| "latest".into());
        validate_type(&resource_type)?;
        validate_part(&scope, ResourceIDError::InvalidScope)?;
        validate_part(&name, ResourceIDError::InvalidName)?;
        validate_tag(&tag)?;
        Ok(Self {
            resource_type,
            scope,
            name,
            tag,
        })
    }

    pub fn find_entities(
        &self,
        world: &core_plugin::World,
    ) -> Result<Vec<core_plugin::Entity>, crate::ResourceError> {
        crate::find_resource_entities(world, self)
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

impl FromStr for ResourceID {
    type Err = ResourceIDError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(ResourceIDError::Empty);
        }
        let (resource_type, remainder) = value
            .split_once(':')
            .ok_or(ResourceIDError::InvalidFormat)?;
        let (scope, name_and_tag) = remainder
            .split_once('/')
            .ok_or(ResourceIDError::InvalidFormat)?;
        if name_and_tag.contains('/') {
            return Err(ResourceIDError::InvalidFormat);
        }
        let (name, tag) = match name_and_tag.split_once(':') {
            Some((name, tag)) if !tag.contains(':') => (name, Some(tag.to_owned())),
            Some(_) => return Err(ResourceIDError::InvalidFormat),
            None => (name_and_tag, None),
        };
        Self::new(resource_type, scope, name, tag)
    }
}

impl fmt::Display for ResourceID {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}/{}:{}",
            self.resource_type, self.scope, self.name, self.tag
        )
    }
}

impl Serialize for ResourceID {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ResourceID {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

pub(crate) fn validate_type(value: &str) -> Result<(), ResourceIDError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
    {
        Err(ResourceIDError::InvalidType)
    } else {
        Ok(())
    }
}

fn validate_part(value: &str, error: ResourceIDError) -> Result<(), ResourceIDError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        Err(error)
    } else {
        Ok(())
    }
}

fn validate_tag(value: &str) -> Result<(), ResourceIDError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(ResourceIDError::InvalidTag);
    };
    if !first.is_ascii_alphanumeric()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        Err(ResourceIDError::InvalidTag)
    } else {
        Ok(())
    }
}
