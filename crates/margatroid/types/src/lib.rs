use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceNameError {
    Empty,
    InvalidScope,
    InvalidName,
    InvalidCharacter,
}

impl fmt::Display for ResourceNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("resource name cannot be empty"),
            Self::InvalidScope => formatter.write_str("resource scope is invalid"),
            Self::InvalidName => formatter.write_str("resource name is invalid"),
            Self::InvalidCharacter => {
                formatter.write_str("resource name contains an invalid character")
            }
        }
    }
}

impl std::error::Error for ResourceNameError {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceName {
    scope: String,
    name: String,
}

impl ResourceName {
    pub fn new(value: impl Into<String>) -> Result<Self, ResourceNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ResourceNameError::Empty);
        }

        let mut parts = value.split('/');
        let scope = parts.next().ok_or(ResourceNameError::InvalidScope)?;
        let name = parts.next().ok_or(ResourceNameError::InvalidName)?;
        if parts.next().is_some() {
            return Err(ResourceNameError::InvalidName);
        }
        validate_part(scope).map_err(|error| match error {
            ResourceNameError::InvalidName => ResourceNameError::InvalidScope,
            error => error,
        })?;
        validate_part(name)?;

        Ok(Self {
            scope: scope.to_owned(),
            name: name.to_owned(),
        })
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for ResourceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.scope, self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentImageReferenceError {
    InvalidName,
    InvalidTag,
}

impl fmt::Display for AgentImageReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => formatter.write_str("agent image name is invalid"),
            Self::InvalidTag => formatter.write_str("agent image tag is invalid"),
        }
    }
}

impl std::error::Error for AgentImageReferenceError {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentImageReference {
    resource: ResourceName,
    tag: String,
}

impl AgentImageReference {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentImageReferenceError> {
        let value = value.into();
        let (resource, tag) = match value.split_once(':') {
            Some((resource, tag)) => (resource, tag),
            None => (value.as_str(), "latest"),
        };
        let resource =
            ResourceName::new(resource).map_err(|_| AgentImageReferenceError::InvalidName)?;
        validate_tag(tag)?;
        Ok(Self {
            resource,
            tag: tag.to_owned(),
        })
    }

    pub fn resource(&self) -> &ResourceName {
        &self.resource
    }

    pub fn scope(&self) -> &str {
        self.resource.scope()
    }

    pub fn name(&self) -> &str {
        self.resource.name()
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }
}

impl fmt::Display for AgentImageReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.resource, self.tag)
    }
}

fn validate_part(part: &str) -> Result<(), ResourceNameError> {
    if part.is_empty() || part == "." || part == ".." {
        return Err(ResourceNameError::InvalidName);
    }
    if part
        .chars()
        .any(|character| character.is_control() || character == '\\')
    {
        return Err(ResourceNameError::InvalidCharacter);
    }
    Ok(())
}

fn validate_tag(tag: &str) -> Result<(), AgentImageReferenceError> {
    if tag.is_empty() || tag.len() > 128 {
        return Err(AgentImageReferenceError::InvalidTag);
    }
    let mut characters = tag.chars();
    let first = characters
        .next()
        .ok_or(AgentImageReferenceError::InvalidTag)?;
    if first == '.' || first == '-' || !is_tag_character(first) {
        return Err(AgentImageReferenceError::InvalidTag);
    }
    if !characters.all(is_tag_character) {
        return Err(AgentImageReferenceError::InvalidTag);
    }
    Ok(())
}

fn is_tag_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_names_are_split_into_scope_and_name() {
        let name = ResourceName::new("local/code-review").unwrap();

        assert_eq!(name.scope(), "local");
        assert_eq!(name.name(), "code-review");
        assert_eq!(name.to_string(), "local/code-review");
    }

    #[test]
    fn resource_names_reject_path_traversal_and_extra_segments() {
        assert!(ResourceName::new("../review").is_err());
        assert!(ResourceName::new("local/../review").is_err());
        assert!(ResourceName::new("local/review/extra").is_err());
    }

    #[test]
    fn resource_names_report_invalid_characters() {
        assert_eq!(
            ResourceName::new("local/bad\\name"),
            Err(ResourceNameError::InvalidCharacter)
        );
    }

    #[test]
    fn agent_image_references_default_to_latest() {
        let reference = AgentImageReference::new("local/coder").unwrap();

        assert_eq!(reference.scope(), "local");
        assert_eq!(reference.name(), "coder");
        assert_eq!(reference.tag(), "latest");
        assert_eq!(reference.to_string(), "local/coder:latest");
    }

    #[test]
    fn agent_image_references_preserve_explicit_tags() {
        let reference = AgentImageReference::new("local/coder:v1.2-rc_1").unwrap();

        assert_eq!(
            reference.resource(),
            &ResourceName::new("local/coder").unwrap()
        );
        assert_eq!(reference.tag(), "v1.2-rc_1");
    }

    #[test]
    fn agent_image_references_reject_invalid_names_and_tags() {
        assert_eq!(
            AgentImageReference::new("coder:latest"),
            Err(AgentImageReferenceError::InvalidName)
        );
        assert_eq!(
            AgentImageReference::new("local/coder:-latest"),
            Err(AgentImageReferenceError::InvalidTag)
        );
        assert_eq!(
            AgentImageReference::new("local/coder:tag:extra"),
            Err(AgentImageReferenceError::InvalidTag)
        );
    }
}
