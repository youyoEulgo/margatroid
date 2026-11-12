use crate::{handler, projection, types::*};
use agent_core::Agent;
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use margatroid_protocol::{ServerMessage, encode_server_message};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use storage_core::{AgentStorage, RecordRange, RecordWatermarks};
use tokio_util::sync::CancellationToken;

pub(crate) fn reconcile_record_watches(world: &mut World) {
    let lifetime = world.get_resource::<DtoState>().unwrap().lifetime.clone();
    let desired = if lifetime.is_cancelled() {
        BTreeMap::new()
    } else {
        world
            .query_with::<Agent>()
            .result()
            .into_iter()
            .filter_map(|entity| {
                Some((
                    entity,
                    (
                        world.get_component::<AgentStorage>(entity)?.clone(),
                        projection::address(world, entity)?,
                    ),
                ))
            })
            .collect::<BTreeMap<_, _>>()
    };
    world
        .get_resource_mut::<SyncState>()
        .unwrap()
        .records
        .retain(|entity, slot| {
            desired
                .get(entity)
                .is_some_and(|(storage, _)| storage.same_database(&slot.storage))
        });
    for (entity, (storage, address)) in desired {
        if world
            .get_resource::<SyncState>()
            .unwrap()
            .records
            .contains_key(&entity)
        {
            continue;
        }
        let cancel = CancellationToken::new();
        let mut changes = storage_core::subscribe_record_changes(&storage);
        let position = *changes.borrow_and_update();
        let context = RecordMonitorContext {
            changes,
            position,
            storage: storage.clone(),
            address,
            events: world.event_sender(),
            cancel: cancel.clone(),
            lifetime: lifetime.clone(),
            guard: lifetime.track(),
        };
        world
            .get_resource_mut::<SyncState>()
            .unwrap()
            .records
            .insert(entity, RecordWatchSlot { storage, cancel });
        world.send_async_closure(RuntimePlugin::UPDATE, move || run_record_watch(context));
    }
}
struct RecordBatch {
    messages: Vec<storage_core::StoredHistoryRecord>,
    mcl: Vec<storage_core::StoredHistoryRecord>,
    logs: Vec<storage_core::StoredAgentLog>,
}
fn read_batch(
    storage: &AgentStorage,
    position: RecordWatermarks,
    high: RecordWatermarks,
) -> Result<RecordBatch, storage_core::StorageError> {
    let range = |after, high: u64| RecordRange {
        after_sequence: Some(after),
        before_sequence: high.checked_add(1).filter(|v| *v <= i64::MAX as u64),
        limit: 50,
    };
    Ok(RecordBatch {
        messages: if high.message > position.message {
            storage_core::read_message_page(storage, range(position.message, high.message))?.records
        } else {
            vec![]
        },
        mcl: if high.mcl > position.mcl {
            storage_core::read_mcl_page(storage, range(position.mcl, high.mcl))?.records
        } else {
            vec![]
        },
        logs: if high.agent_log > position.agent_log {
            storage_core::read_agent_log_page(storage, range(position.agent_log, high.agent_log))?
                .records
        } else {
            vec![]
        },
    })
}
fn encode_batch(address: &str, batch: &RecordBatch) -> Result<Vec<Value>, DtoAsyncError> {
    let mut frames = vec![];
    for row in &batch.messages {
        frames.push(json!({"type":"agent.message.full.sync","address":address,"turn":row.record.turn,"generation":row.record.payload.get("request_id").and_then(Value::as_str),"history_sequence":row.sequence,"message":row.record.payload,"error":null}));
    }
    if !batch.mcl.is_empty() {
        frames.push(json!({"type":"agent.mcl.sync","address":address,"records":batch.mcl.iter().cloned().map(handler::mcl_record).collect::<Vec<_>>()}));
    }
    if !batch.logs.is_empty() {
        frames.push(json!({"type":"agent.log.sync","address":address,"records":batch.logs.iter().cloned().map(handler::log_record).collect::<Vec<_>>()}));
    }
    for frame in &frames {
        let value: ServerMessage =
            serde_json::from_value(frame.clone()).map_err(|e| DtoAsyncError(e.to_string()))?;
        encode_server_message(&value).map_err(|e| DtoAsyncError(e.to_string()))?;
    }
    Ok(frames)
}
async fn run_record_watch(mut context: RecordMonitorContext) -> Result<(), DtoAsyncError> {
    let _guard = context.guard;
    let mut position = context.position;
    loop {
        tokio::select! {
            biased;
            _=context.cancel.cancelled()=>return Ok(()),
            _=context.lifetime.cancelled()=>return Ok(()),
            changed=context.changes.changed()=>if changed.is_err(){return Ok(());},
        }
        let high = *context.changes.borrow_and_update();
        let mut previous_error = None;
        while position != high {
            if context.cancel.is_cancelled() || context.lifetime.is_cancelled() {
                return Ok(());
            }
            let storage = context.storage.clone();
            let address = context.address.clone();
            let read = tokio::task::spawn_blocking(move || {
                let batch = read_batch(&storage, position, high)
                    .map_err(|e| DtoAsyncError(e.to_string()))?;
                let frames = encode_batch(&address, &batch)?;
                Ok::<_, DtoAsyncError>((batch, frames))
            })
            .await
            .map_err(|e| DtoAsyncError(e.to_string()))
            .and_then(|v| v);
            let (batch, frames) = match read {
                Ok(value) => {
                    previous_error = None;
                    value
                }
                Err(error) => {
                    if previous_error.as_ref() != Some(&error.0) {
                        tracing::warn!(reason=%error.0,"record synchronization read failed");
                        previous_error = Some(error.0);
                    }
                    tokio::select! {
                        biased;
                        _=context.cancel.cancelled()=>return Ok(()),
                        _=context.lifetime.cancelled()=>return Ok(()),
                        _=tokio::time::sleep(Duration::from_millis(250))=>(),
                    }
                    continue;
                }
            };
            if context.cancel.is_cancelled() || context.lifetime.is_cancelled() {
                return Ok(());
            }
            for message in frames {
                context.events.send_event(WebSocketMessageSend {
                    target: config_core::OutboundTarget::Broadcast,
                    message,
                });
            }
            position.message = batch
                .messages
                .last()
                .map(|r| r.sequence)
                .unwrap_or(high.message);
            position.mcl = batch.mcl.last().map(|r| r.sequence).unwrap_or(high.mcl);
            position.agent_log = batch
                .logs
                .last()
                .map(|r| r.sequence)
                .unwrap_or(high.agent_log);
        }
    }
}
