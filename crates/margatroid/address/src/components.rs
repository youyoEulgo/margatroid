use core_plugin::Component;

use crate::AddressError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    value: String,
}

impl Address {
    pub fn new(value: &str) -> Result<Self, AddressError> {
        if !value.starts_with('/')
            || value.contains(['\\', ':'])
            || value.chars().any(char::is_control)
            || (value != "/"
                && value[1..]
                    .split('/')
                    .any(|segment| segment.is_empty() || matches!(segment, "." | "..")))
        {
            return Err(AddressError::InvalidNodePath);
        }
        Ok(Self {
            value: value.to_owned(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl Component for Address {}
