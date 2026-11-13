use crate::{handler, projection, types::*};
use agent_core::Agent;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use margatroid_protocol::{ServerMessage, encode_server_message};
use serde_json::{Value, json};
use std::time::Duration;
use storage_core::{AgentStorage, RecordRange, RecordWatermarks};
use tokio_util::sync::CancellationToken;

pub(crate) fn refresh_record_watch(
    world: &mut World,
    entity: core_plugin::Entity,
) -> Vec<agent_core::AgentObservationWait> {
    let state = world.get_resource::<DtoState>().unwrap().clone();
    let desired = if state.lifetime.is_cancelled() || world.get_component::<Agent>(entity).is_none()
    {
        None
    } else {
        world
            .get_component::<AgentStorage>(entity)
            .cloned()
            .zip(projection::address(world, entity))
    };
    let same = world
        .get_resource::<SyncObservations>()
        .unwrap()
        .records
        .get(&entity)
        .is_some_and(|slot| {
            desired
                .as_ref()
                .is_some_and(|(storage, _)| storage.same_database(&slot.storage))
        });
    if same {
        return vec![];
    }
    let mut waits = vec![];
    if let Some(slot) = world
        .get_resource_mut::<SyncObservations>()
        .unwrap()
        .records
        .remove(&entity)
    {
        waits.push(slot.completion.clone().wait());
        drop(slot);
    }
    let Some((storage, address)) = desired else {
        return waits;
    };
    let cancel = CancellationToken::new();
    let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let completion = std::sync::Arc::new(crate::monitor::MonitorCompletion::default());
    let mut changes = storage_core::subscribe_record_changes(&storage);
    let position = *changes.borrow_and_update();
    let context = RecordMonitorContext {
        changes,
        position,
        storage: storage.clone(),
        address,
        clients: state.clients.clone(),
        targets: crate::sync_selectors(
            world
                .get_resource::<config_core::MargatroidConfig>()
                .unwrap()
                .backend_state(),
        ),
        active: active.clone(),
        cancel: cancel.clone(),
        lifetime: state.lifetime.clone(),
        guard: state.lifetime.track(),
        completion: crate::monitor::MonitorCompletionGuard(completion.clone()),
    };
    world
        .get_resource_mut::<SyncObservations>()
        .unwrap()
        .records
        .insert(
            entity,
            RecordWatchSlot {
                storage,
                cancel,
                active,
                completion,
            },
        );
    world.send_async_closure(RuntimePlugin::UPDATE, move || run_record_watch(context));
    waits
}
pub(crate) fn stop_record_watch(
    world: &mut World,
    entity: core_plugin::Entity,
) -> Vec<agent_core::AgentObservationWait> {
    if let Some(slot) = world
        .get_resource_mut::<SyncObservations>()
        .unwrap()
        .records
        .remove(&entity)
    {
        let wait = slot.completion.clone().wait();
        drop(slot);
        vec![wait]
    } else {
        vec![]
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
fn encode_batch(
    address: &str,
    batch: &RecordBatch,
) -> Result<Vec<std::sync::Arc<client_core::ClientFrame>>, DtoAsyncError> {
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
    frames
        .into_iter()
        .map(|frame| {
            let value: ServerMessage =
                serde_json::from_value(frame).map_err(|e| DtoAsyncError(e.to_string()))?;
            encode_server_message(&value)
                .map(|text| std::sync::Arc::new(client_core::ClientFrame::Text(text)))
                .map_err(|e| DtoAsyncError(e.to_string()))
        })
        .collect()
}
async fn run_record_watch(mut context: RecordMonitorContext) -> Result<(), DtoAsyncError> {
    let _guard = context.guard;
    let _completion = context.completion;
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
            for frame in frames {
                context
                    .clients
                    .send_scoped(&context.targets, frame, context.active.clone());
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
