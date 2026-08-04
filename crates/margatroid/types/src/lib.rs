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
}
