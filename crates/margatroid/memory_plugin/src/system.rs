use core_plugin::World;
use margatroid_types::{AgentHistoryMessageWriteRequested, AgentHistoryRecordWriteRequested, AgentSettingWriteRequested};

use crate::events::AgentMemoryWriteFailed;
use crate::handler::{handle_history_message_write, handle_history_record_write, handle_setting_write};

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
