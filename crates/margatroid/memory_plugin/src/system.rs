use core_plugin::World;
use margatroid_types::{
    AgentHistoryMessageWriteRequested, AgentRealtimeContextReadCompleted,
    AgentRealtimeContextReadRequested, AgentRealtimeContextWriteRequested,
    AgentHistoryRecordWriteRequested, AgentSettingWriteRequested,
};

use crate::events::AgentMemoryWriteFailed;
use crate::handler::{
    handle_history_message_write, handle_realtime_context_read, handle_realtime_context_write,
    handle_history_record_write, handle_setting_write,
};

pub(crate) fn sync_history_messages_system(world: &mut World) {
    let events = world
        .event_reader::<AgentHistoryMessageWriteRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        if let Err(error) = handle_history_message_write(world, &event) {
            world.emit_event(AgentMemoryWriteFailed {
                agent: event.agent,
                error,
            });
        }
    }
}

pub(crate) fn sync_history_records_system(world: &mut World) {
    let events = world
        .event_reader::<AgentHistoryRecordWriteRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        if let Err(error) = handle_history_record_write(world, &event) {
            world.emit_event(AgentMemoryWriteFailed {
                agent: event.agent,
                error,
            });
        }
    }
}

pub(crate) fn sync_settings_system(world: &mut World) {
    let events = world
        .event_reader::<AgentSettingWriteRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        if let Err(error) = handle_setting_write(world, &event) {
            world.emit_event(AgentMemoryWriteFailed {
                agent: event.agent,
                error,
            });
        }
    }
}

pub(crate) fn sync_realtime_context_system(world: &mut World) {
    let events = world
        .event_reader::<AgentRealtimeContextWriteRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        if let Err(error) = handle_realtime_context_write(world, &event) {
            world.emit_event(AgentMemoryWriteFailed {
                agent: event.agent,
                error,
            });
        }
    }
}

pub(crate) fn read_realtime_context_system(world: &mut World) {
    let requests = world
        .event_reader::<AgentRealtimeContextReadRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let result = handle_realtime_context_read(world, &request);
        world.emit_event(AgentRealtimeContextReadCompleted {
            id: request.id,
            agent: request.agent,
            result: result.map_err(|error| error.to_string()),
        });
    }
}
