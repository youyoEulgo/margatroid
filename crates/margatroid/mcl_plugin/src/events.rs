use core_plugin::{Entity, Event};
use margatroid_types::ResourceId;

use crate::{MclCommandId, MclCommandReply, MclError, MclOperation};

#[derive(Clone, Debug)]
pub struct MclCommandRequest {
    pub id: MclCommandId,
    pub agent_id: ResourceId,
    pub command: String,
    pub binding: Option<serde_json::Value>,
    pub reply: MclCommandReply,
}
impl Event for MclCommandRequest {}

#[derive(Clone, Debug)]
pub struct MclDomainRequest {
    pub id: MclCommandId,
    pub agent_id: ResourceId,
    pub operation: MclOperation,
    pub reply: MclCommandReply,
}
impl Event for MclDomainRequest {}

#[derive(Clone, Debug)]
pub struct MclDomainResponse {
    pub id: MclCommandId,
    pub agent_id: ResourceId,
    pub result: Result<crate::MclDomainValue, MclError>,
    pub reply: MclCommandReply,
}

#[derive(Clone, Debug)]
pub struct MclImportState {
    pub command_id: MclCommandId,
    pub agent_id: ResourceId,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: String,
    pub reply: MclCommandReply,
}

#[derive(Clone, Debug)]
pub struct MclEffectState {
    pub command_id: MclCommandId,
    pub agent_id: ResourceId,
    pub agent: Entity,
    pub vm_id: Option<margatroid_types::LuaVmId>,
    pub kind: crate::MclPendingEffectKind,
    pub reply: MclCommandReply,
}
impl Event for MclDomainResponse {}
