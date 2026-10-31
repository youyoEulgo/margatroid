mod components;
mod error;
mod handler;
pub mod types;

use core_plugin::{Entity, World};

pub use components::Address;
pub use error::AddressError;
pub use handler::resolve_file;
pub use types::ParsedAddress;

pub trait WorldAddressExt {
    fn entity_by_address(&self, address: &str) -> Result<Entity, AddressError>;
    fn address_exists(&self, address: &str) -> Result<bool, AddressError>;
}

impl WorldAddressExt for World {
    fn entity_by_address(&self, address: &str) -> Result<Entity, AddressError> {
        let address = Address::new(address)?;
        let mut matches = self
            .query_with::<Address>()
            .result()
            .into_iter()
            .filter(|entity| self.get_component::<Address>(*entity) == Some(&address));
        let entity = matches.next().ok_or(AddressError::EntityMissing)?;
        if matches.next().is_some() {
            return Err(AddressError::DuplicateAddress);
        }
        Ok(entity)
    }

    fn address_exists(&self, address: &str) -> Result<bool, AddressError> {
        let address = Address::new(address)?;
        Ok(self
            .query_with::<Address>()
            .result()
            .into_iter()
            .any(|entity| self.get_component::<Address>(entity) == Some(&address)))
    }
}
