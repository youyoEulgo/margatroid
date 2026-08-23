use core_plugin::{Entity, Event};
use margatroid_types::ResourceId;

use crate::{ResourceMapEntry, ToolError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRegisterRequest {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: Option<String>,
}
impl Event for ToolRegisterRequest {}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolRegisterResponse {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: Option<String>,
    pub result: Result<ResourceMapEntry, ToolError>,
}
impl Event for ToolRegisterResponse {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelToolTurn {
    pub turn_id: String,
    pub agent: Entity,
}
impl Event for CancelToolTurn {}

pub use margatroid_types::ToolCallEvent;
