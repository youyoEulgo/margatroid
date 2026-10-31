use core_plugin::Component;

use crate::{AddressError, ParsedAddress};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    value: String,
}

impl Address {
    pub fn new(value: &str) -> Result<Self, AddressError> {
        Ok(Self {
            value: ParsedAddress::parse(value)?.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl Component for Address {}
