use std::fmt;

use core_plugin::Entity;
use resource_id_plugin::{ResourceIdAttachError, ResourceIdLookupError, ResourceRef};

#[derive(Debug)]
pub enum ResourceLibraryError {
    OwnerMissing(Entity),
    TypeMismatch,
    InvalidHost,
    Lookup(ResourceIdLookupError),
    Attach(ResourceIdAttachError),
    WrongType {
        reference: ResourceRef,
        expected: &'static str,
    },
    ContentUnreadable {
        reference: ResourceRef,
        reason: String,
    },
}

impl fmt::Display for ResourceLibraryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OwnerMissing(owner) => write!(formatter, "owner {owner:?} is not alive"),
            Self::TypeMismatch => {
                formatter.write_str("resource type does not match its definition")
            }
            Self::InvalidHost => formatter.write_str("resource host is not a resource entity"),
            Self::Lookup(error) => write!(formatter, "{error:?}"),
            Self::Attach(error) => write!(formatter, "{error:?}"),
            Self::WrongType {
                reference,
                expected,
            } => {
                write!(formatter, "{reference} is not a {expected} resource")
            }
            Self::ContentUnreadable { reference, reason } => {
                write!(formatter, "{reference} content could not be read: {reason}")
            }
        }
    }
}

impl std::error::Error for ResourceLibraryError {}
