use std::fmt;

use crate::AddressError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedAddress {
    pub kind: String,
    pub scope: String,
    pub name: String,
    pub tag: String,
}

impl ParsedAddress {
    pub fn parse(value: &str) -> Result<Self, AddressError> {
        let (kind, remainder) = value.split_once(':').ok_or(AddressError::InvalidFormat)?;
        let (scope, name_and_tag) = remainder
            .split_once('/')
            .ok_or(AddressError::InvalidFormat)?;
        if name_and_tag.contains('/') {
            return Err(AddressError::InvalidFormat);
        }
        let (name, tag) = name_and_tag
            .split_once(':')
            .unwrap_or((name_and_tag, "latest"));
        if kind.is_empty()
            || !kind.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
        {
            return Err(AddressError::InvalidPart);
        }
        for part in [scope, name, tag] {
            if part.is_empty()
                || matches!(part, "." | "..")
                || part.chars().any(|character| {
                    character.is_control() || matches!(character, '/' | '\\' | ':')
                })
            {
                return Err(AddressError::InvalidPart);
            }
        }
        Ok(Self {
            kind: kind.to_owned(),
            scope: scope.to_owned(),
            name: name.to_owned(),
            tag: tag.to_owned(),
        })
    }
}

impl fmt::Display for ParsedAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}/{}:{}",
            self.kind, self.scope, self.name, self.tag
        )
    }
}
