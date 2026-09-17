use agent_plugin::Agent;
use core_plugin::World;
use margatroid_types::{
    AgentHistoryMessageWriteRequested, AgentRealtimeContextReadRequested,
    AgentHistoryRecordWriteRequested, AgentSettingWriteRequested,
    AgentRealtimeContextWriteRequested, MclMessage,
};

use crate::error::{MemoryError, MemoryErrorKind};

pub(crate) fn handle_store_error(error: agent_plugin::AgentMemoryStoreError) -> MemoryError {
    MemoryError::new(
        match error.kind.as_str() {
            "WriteFailed" => MemoryErrorKind::WriteFailed,
            "ReadFailed" => MemoryErrorKind::ReadFailed,
            _ => MemoryErrorKind::AgentMemoryMissing,
        },
        error.message,
    )
}

pub(crate) fn handle_history_message_write(
    world: &World,
    event: &AgentHistoryMessageWriteRequested,
) -> Result<(), MemoryError> {
    if !world.is_alive(event.agent) {
        return Err(MemoryError::new(
            MemoryErrorKind::AgentNotAlive,
            "agent entity is not alive",
        ));
    }
    let agent = world.get_component::<Agent>(event.agent).ok_or_else(|| {
        MemoryError::new(
            MemoryErrorKind::AgentMemoryMissing,
            "agent does not have memory",
        )
    })?;
    agent
        .memory
        .append_history(&event.id, &event.source, &event.message, &event.tool_schema, event.usage.as_ref(),
        )
        .map_err(handle_store_error)
}

pub(crate) fn handle_history_record_write(
    world: &World,
    event: &AgentHistoryRecordWriteRequested,
) -> Result<(), MemoryError> {
    let agent = world.get_component::<Agent>(event.agent).ok_or_else(|| {
        MemoryError::new(
            MemoryErrorKind::AgentMemoryMissing,
            "agent does not have memory",
        )
    })?;
    agent
        .memory
        .append_record(&event.kind, &event.content, &event.payload, &event.source)
        .map_err(handle_store_error)
}

pub(crate) fn handle_setting_write(
    world: &World,
    event: &AgentSettingWriteRequested,
) -> Result<(), MemoryError> {
    let agent = world.get_component::<Agent>(event.agent).ok_or_else(|| {
        MemoryError::new(
            MemoryErrorKind::AgentMemoryMissing,
            "agent does not have memory",
        )
    })?;
    let entries = event
        .entries
        .iter()
        .map(|entry| (entry.key.clone(), entry.value.clone()))
        .collect::<Vec<_>>();
    agent.memory.set_setting(&entries).map_err(handle_store_error)
}

pub(crate) fn handle_realtime_context_write(
    world: &World,
    event: &AgentRealtimeContextWriteRequested,
) -> Result<(), MemoryError> {
    if !world.is_alive(event.agent) {
        return Err(MemoryError::new(
            MemoryErrorKind::AgentNotAlive,
            "agent entity is not alive",
        ));
    }
    let agent = world.get_component::<Agent>(event.agent).ok_or_else(|| {
        MemoryError::new(
            MemoryErrorKind::AgentMemoryMissing,
            "agent does not have memory",
        )
    })?;
    agent
        .memory
        .rewrite_realtime(&event.messages)
        .map_err(handle_store_error)
}

pub(crate) fn handle_realtime_context_read(
    world: &World,
    request: &AgentRealtimeContextReadRequested,
) -> Result<Vec<MclMessage>, MemoryError> {
    let agent = world.get_component::<Agent>(request.agent).ok_or_else(|| {
        MemoryError::new(
            MemoryErrorKind::AgentMemoryMissing,
            "agent does not have memory",
        )
    })?;
    agent.memory.read_realtime().map_err(handle_store_error)
}
