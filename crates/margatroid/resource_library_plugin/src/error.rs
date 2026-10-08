use std::fmt;

use core_plugin::Entity;
use resource_id_plugin::{ResourceIdAttachError, ResourceIdLookupError};

#[derive(Debug)]
pub enum ResourceLibraryError {
    OwnerMissing(Entity),
    TypeMismatch,
    InvalidHost,
    Lookup(ResourceIdLookupError),
    Attach(ResourceIdAttachError),
}

impl fmt::Display for ResourceLibraryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ResourceLibraryError {}
