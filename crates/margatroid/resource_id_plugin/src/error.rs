use std::any::TypeId;
use std::fmt;

use crate::ResourceID;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceError {
    PluginMissing,
    TypeNotRegistered {
        resource_type: String,
    },
    InvalidResourceType {
        resource_type: String,
    },
    TypeRegistrationConflict {
        resource_type: String,
    },
    EntityNotAlive {
        entity: Entity,
    },
    ComponentNotRegistered {
        component_type: TypeId,
    },
    ComponentKindMismatch {
        component_type: TypeId,
    },
    AddressTypeMismatch {
        id: ResourceID,
        expected: String,
    },
    DuplicateEntryID {
        id: ResourceID,
    },
    LibraryConflict {
        entity: Entity,
    },
    LibraryAddressOccupied {
        id: ResourceID,
        entities: Vec<Entity>,
    },
}

impl fmt::Display for ResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PluginMissing => formatter.write_str("ResourcePlugin or registry is missing"),
            Self::TypeNotRegistered { resource_type } => write!(
                formatter,
                "resource type `{resource_type}` is not registered"
            ),
            Self::InvalidResourceType { resource_type } => {
                write!(formatter, "resource type `{resource_type}` is invalid")
            }
            Self::TypeRegistrationConflict { resource_type } => write!(
                formatter,
                "resource type `{resource_type}` registration conflicts"
            ),
            Self::EntityNotAlive { entity } => write!(formatter, "entity {entity:?} is not alive"),
            Self::ComponentNotRegistered { component_type } => {
                write!(formatter, "component {component_type:?} is not registered")
            }
            Self::ComponentKindMismatch { component_type } => write!(
                formatter,
                "component {component_type:?} has a different resource kind"
            ),
            Self::AddressTypeMismatch { id, expected } => {
                write!(formatter, "address `{id}` must have type `{expected}`")
            }
            Self::DuplicateEntryID { id } => write!(formatter, "duplicate resource entry `{id}`"),
            Self::LibraryConflict { entity } => write!(
                formatter,
                "entity {entity:?} already has another library component"
            ),
            Self::LibraryAddressOccupied { id, entities } => write!(
                formatter,
                "library address `{id}` is occupied by {entities:?}"
            ),
        }
    }
}

impl std::error::Error for ResourceError {}

use core_plugin::Entity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceIDError {
    Empty,
    InvalidType,
    InvalidScope,
    InvalidName,
    InvalidTag,
    InvalidFormat,
}

impl fmt::Display for ResourceIDError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "resource id cannot be empty",
            Self::InvalidType => "resource type is invalid",
            Self::InvalidScope => "resource scope is invalid",
            Self::InvalidName => "resource name is invalid",
            Self::InvalidTag => "resource tag is invalid",
            Self::InvalidFormat => "resource id format is invalid",
        })
    }
}

impl std::error::Error for ResourceIDError {}
