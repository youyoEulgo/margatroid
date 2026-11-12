use crate::{handler, projection, types::*};
use agent_core::{Agent, AgentDriverSource};
use agent_driver_core::{
    DriverUiBundle, DriverUiError, DriverUiState, open_driver_ui_watch, run_driver_ui_monitor,
};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::WorldAsyncExt;
use base64::{Engine, engine::general_purpose::STANDARD};
use client_core::ClientFrame;
use closure_plugin::WorldClosureExt;
use core_plugin::World;
use futures_util::FutureExt;
use margatroid_protocol::*;
use std::{
    any::Any,
    collections::BTreeMap,
    panic::AssertUnwindSafe,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

fn ui_error(value: DriverUiError) -> ProtocolErrorDto {
    let (code, message) = match value {
        DriverUiError::Initializing | DriverUiError::Unstable => {
            ("AgentUiNotReady", "Driver UI is not ready")
        }
        DriverUiError::EntryMissing => ("DriverUiEntryMissing", "Driver UI entry is missing"),
        DriverUiError::PathInvalid => (
            "DriverUiPathInvalid",
            "Driver UI contains an invalid resource path",
        ),
        DriverUiError::TooLarge => ("DriverUiTooLarge", "Driver UI exceeds the message budget"),
        DriverUiError::ReadFailed(_) => ("DriverUiReadFailed", "Driver UI could not be read"),
    };
    ProtocolErrorDto::new(code, message)
}
pub(crate) fn project_agent_ui(
    value: Option<Arc<DriverUiBundle>>,
) -> Result<Option<AgentUiBundleDto>, ProtocolErrorDto> {
    value
        .map(|bundle| {
            let value = AgentUiBundleDto {
                revision: bundle.revision.clone(),
                entry: bundle.entry.clone(),
                files: bundle
                    .files
                    .iter()
                    .map(|file| AgentUiFileDto {
                        path: file.path.clone(),
                        media_type: file.media_type.clone(),
                        data: STANDARD.encode(&file.data),
                    })
                    .collect(),
            };
            validate_ui_bundle(&value).map_err(|_| ui_error(DriverUiError::PathInvalid))?;
            Ok(value)
        })
        .transpose()
}
pub(crate) fn check_ui_budget(
    address: &str,
    ui: &Option<AgentUiBundleDto>,
) -> Result<(), ProtocolErrorDto> {
    let probe = ServerMessage::AgentUiRes {
        id: "\\".repeat(MAX_ID_BYTES),
        success: true,
        address: address.to_owned(),
        ui: ui.clone(),
        error: None,
    };
    encode_server_message(&probe).map_err(|_| ui_error(DriverUiError::TooLarge))?;
    Ok(())
}
fn build_ui_delivery(address: &str, state: DriverUiState) -> UiDelivery {
    let initializing = matches!(state, DriverUiState::Initializing);
    let result = match state {
        DriverUiState::Ready(value) => project_agent_ui(value).and_then(|ui| {
            check_ui_budget(address, &ui)?;
            Ok(ui)
        }),
        DriverUiState::Initializing => Err(ui_error(DriverUiError::Initializing)),
        DriverUiState::Failed(error) => Err(ui_error(error)),
    };
    let (key, ui, error) = match result {
        Ok(Some(ui)) => (UiDeliveryKey::Ready(ui.revision.clone()), Some(ui), None),
        Ok(None) => (UiDeliveryKey::Missing, None, None),
        Err(error) => {
            let key = if initializing {
                UiDeliveryKey::Initializing
            } else {
                UiDeliveryKey::Failed {
                    code: error.code.clone(),
                    message: error.message.clone(),
                }
            };
            (key, None, Some(error))
        }
    };
    let message = ServerMessage::AgentUiSync {
        address: address.to_owned(),
        ui,
        error,
    };
    let frame = match encode_server_message(&message) {
        Ok(text) => text,
        Err(_) => {
            let failure = ServerMessage::AgentUiSync {
                address: address.to_owned(),
                ui: None,
                error: Some(ui_error(DriverUiError::TooLarge)),
            };
            return UiDelivery {
                key: UiDeliveryKey::Failed {
                    code: "DriverUiTooLarge".into(),
                    message: "Driver UI exceeds the message budget".into(),
                },
                frame: Arc::from(encode_server_message(&failure).expect("bounded UI error frame")),
            };
        }
    };
    UiDelivery {
        key,
        frame: Arc::from(frame),
    }
}
pub(crate) fn query_agent_ui(world: &World, object: Box<dyn Any + Send>) {
    let Ok(request) = object.downcast::<Prepared<AgentTargetDto, Option<Arc<DriverUiBundle>>>>()
    else {
        return;
    };
    let result = (|| {
        handler::client(world, request.ticket.connection)?;
        let entity = handler::agent(world, &request.input.address)
            .map_err(|_| ProtocolErrorDto::new("AgentMissing", "Agent is not active"))?;
        let source = world
            .get_component::<AgentDriverSource>(entity)
            .ok_or_else(|| ui_error(DriverUiError::Initializing))?;
        let slot = world
            .get_resource::<SyncState>()
            .and_then(|state| state.ui.get(&entity))
            .filter(|slot| slot.manifest_path == source.manifest_path)
            .ok_or_else(|| ui_error(DriverUiError::Initializing))?;
        match slot.watch.state() {
            DriverUiState::Initializing => Err(ui_error(DriverUiError::Initializing)),
            DriverUiState::Failed(error) => Err(ui_error(error)),
            DriverUiState::Ready(value) => Ok(value),
        }
    })();
    handler::deliver(&request.reply, result);
}
fn wake_ui_frame(pending: Arc<AtomicBool>, _: &mut World) {
    pending.store(false, Ordering::Release);
}
pub(crate) fn reconcile_ui_watches(world: &mut World) {
    let state = world
        .get_resource::<DtoState>()
        .expect("DTO installed")
        .clone();
    if state.lifetime.is_cancelled() {
        let sync = world.get_resource_mut::<SyncState>().unwrap();
        sync.ui.clear();
        sync.ui_sent.clear();
        return;
    }
    let desired = world
        .query_with::<AgentDriverSource>()
        .result()
        .into_iter()
        .filter_map(|entity| {
            world.get_component::<Agent>(entity)?;
            let source = world.get_component::<AgentDriverSource>(entity)?;
            Some((
                entity,
                (
                    source.manifest_path.clone(),
                    projection::address(world, entity)?,
                ),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    {
        let sync = world.get_resource_mut::<SyncState>().unwrap();
        let stale = sync
            .ui
            .iter()
            .filter_map(|(entity, slot)| {
                let current = desired.get(entity);
                if current.is_none_or(|(path, address)| {
                    path != &slot.manifest_path || address != &slot.address
                }) || slot.watch.is_cancelled()
                {
                    Some(*entity)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for entity in stale {
            sync.ui.remove(&entity);
            sync.ui_sent.retain(|(_, current), _| *current != entity);
        }
    }
    for (entity, (manifest_path, address)) in desired {
        if world
            .get_resource::<SyncState>()
            .unwrap()
            .ui
            .contains_key(&entity)
        {
            continue;
        }
        let delivery = Arc::new(RwLock::new(build_ui_delivery(
            &address,
            DriverUiState::Initializing,
        )));
        let frame_pending = Arc::new(AtomicBool::new(false));
        let output = delivery.clone();
        let pending = frame_pending.clone();
        let target = address.clone();
        let frames = world.closure_sender(RuntimePlugin::UPDATE);
        let lifetime = state.lifetime.clone();
        let (watch, monitor) = open_driver_ui_watch(
            manifest_path.clone(),
            Arc::new(move |value| {
                if lifetime.is_cancelled() {
                    return;
                }
                let value = build_ui_delivery(&target, value);
                *output.write().unwrap_or_else(|e| e.into_inner()) = value;
                if !pending.swap(true, Ordering::AcqRel) {
                    let pending = pending.clone();
                    frames.send(move |world| wake_ui_frame(pending, world));
                }
            }),
        );
        let context = UiMonitorContext {
            monitor,
            watch: watch.clone(),
            lifetime: state.lifetime.clone(),
            guard: state.lifetime.track(),
        };
        world.get_resource_mut::<SyncState>().unwrap().ui.insert(
            entity,
            UiWatchSlot {
                manifest_path,
                address,
                watch,
                delivery,
                frame_pending,
            },
        );
        world.send_async_closure(RuntimePlugin::UPDATE, move || run_ui_watch(context));
    }
}
async fn run_ui_watch(context: UiMonitorContext) -> Result<(), DtoAsyncError> {
    let _guard = context.guard;
    let monitor = AssertUnwindSafe(run_driver_ui_monitor(context.monitor)).catch_unwind();
    tokio::pin!(monitor);
    let result = tokio::select! {
        value=&mut monitor=>value,
        _=context.lifetime.cancelled()=>{context.watch.cancel();monitor.await},
    };
    let result =
        result.unwrap_or_else(|_| Err(DriverUiError::ReadFailed("UI monitor panicked".into())));
    if let Err(error) = result {
        context.watch.publish(DriverUiState::Failed(error.clone()));
        context.watch.cancel();
        return Err(DtoAsyncError(error.to_string()));
    }
    Ok(())
}
pub(crate) fn publish_ui(world: &mut World) {
    let targets = world
        .get_resource::<config_core::MargatroidConfig>()
        .map(|config| config.backend_state().to_vec())
        .unwrap_or_default();
    let senders = handler::recipients(world, &targets);
    let deliveries = world
        .get_resource::<SyncState>()
        .unwrap()
        .ui
        .iter()
        .map(|(entity, slot)| {
            (
                *entity,
                slot.delivery
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            )
        })
        .collect::<Vec<_>>();
    let live = senders
        .iter()
        .map(|(connection, _)| *connection)
        .collect::<std::collections::BTreeSet<_>>();
    world
        .get_resource_mut::<SyncState>()
        .unwrap()
        .ui_sent
        .retain(|(connection, _), _| live.contains(connection));
    for (connection, sender) in senders {
        for (entity, delivery) in &deliveries {
            if world
                .get_resource::<SyncState>()
                .unwrap()
                .ui_sent
                .get(&(connection, *entity))
                == Some(&delivery.key)
            {
                continue;
            }
            if sender
                .try_send(ClientFrame::Text(delivery.frame.to_string()))
                .is_ok()
            {
                world
                    .get_resource_mut::<SyncState>()
                    .unwrap()
                    .ui_sent
                    .insert((connection, *entity), delivery.key.clone());
            } else {
                world
                    .get_resource::<ConnState>()
                    .unwrap()
                    .cancel(connection);
                break;
            }
        }
    }
}
