use std::fmt;
use std::str::FromStr;

use core_plugin::Component;
use serde::{Deserialize, Serialize};

use crate::ResourceIdError;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceRef {
    resource_type: String,
    scope: String,
    name: String,
    tag: String,
}

impl ResourceRef {
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

impl FromStr for ResourceRef {
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

impl fmt::Display for ResourceRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}/{}:{}",
            self.resource_type, self.scope, self.name, self.tag
        )
    }
}

impl Serialize for ResourceRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ResourceRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceId(pub ResourceRef);

impl ResourceId {
    pub fn as_ref(&self) -> &ResourceRef {
        &self.0
    }
    pub fn into_inner(self) -> ResourceRef {
        self.0
    }
}

impl From<ResourceRef> for ResourceId {
    fn from(value: ResourceRef) -> Self {
        Self(value)
    }
}

impl AsRef<ResourceRef> for ResourceId {
    fn as_ref(&self) -> &ResourceRef {
        &self.0
    }
}

impl Component for ResourceId {}

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
