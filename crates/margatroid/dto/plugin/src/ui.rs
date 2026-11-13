use crate::{handler, projection, types::*};
use agent_core::{Agent, AgentDriverSource};
use agent_driver_core::{
    DriverUiBundle, DriverUiError, DriverUiState, open_driver_ui_watch, run_driver_ui_monitor,
};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::WorldAsyncExt;
use base64::{Engine, engine::general_purpose::STANDARD};
use client_core::ClientFrame;
use core_plugin::World;
use futures_util::FutureExt;
use margatroid_protocol::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    any::Any,
    panic::AssertUnwindSafe,
    sync::{Arc, RwLock},
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
            .get_resource::<SyncObservations>()
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
pub(crate) fn refresh_ui_watch(
    world: &mut World,
    entity: core_plugin::Entity,
) -> Vec<agent_core::AgentObservationWait> {
    let state = world.get_resource::<DtoState>().unwrap().clone();
    let desired = if state.lifetime.is_cancelled() || world.get_component::<Agent>(entity).is_none()
    {
        None
    } else {
        world
            .get_component::<AgentDriverSource>(entity)
            .and_then(|source| {
                projection::address(world, entity)
                    .map(|address| (source.manifest_path.clone(), address))
            })
    };
    let same = world
        .get_resource::<SyncObservations>()
        .unwrap()
        .ui
        .get(&entity)
        .is_some_and(|slot| {
            desired.as_ref().is_some_and(|(path, address)| {
                *path == slot.manifest_path && *address == slot.address
            })
        });
    if same {
        return vec![];
    }
    let waits = stop_ui_watch(world, entity);
    let Some((manifest_path, address)) = desired else {
        return waits;
    };
    let delivery = Arc::new(RwLock::new(build_ui_delivery(
        &address,
        DriverUiState::Initializing,
    )));
    let output = delivery.clone();
    let revision = Arc::new(AtomicU64::new(0));
    let version = revision.clone();
    let weak = Arc::downgrade(&delivery);
    let targets = crate::sync_selectors(
        world
            .get_resource::<config_core::MargatroidConfig>()
            .unwrap()
            .backend_state(),
    );
    let sync = client_core::SyncContext::new(
        state.clients.clone(),
        client_core::SyncKey {
            topic: "agent.ui.sync".into(),
            object: address.clone(),
        },
        targets,
        move || {
            let delivery = weak
                .upgrade()
                .ok_or(client_core::ClientPrepareError::SourceClosed)?;
            let value = delivery.read().unwrap_or_else(|e| e.into_inner());
            Ok((version.load(Ordering::Acquire), value.clone()))
        },
        |delivery: UiDelivery| Ok(Arc::new(ClientFrame::Text(delivery.frame.to_string()))),
    );
    let publish = sync.clone();
    let target = address.clone();
    let lifetime = state.lifetime.clone();
    let (watch, monitor) = open_driver_ui_watch(
        manifest_path.clone(),
        Arc::new(move |value| {
            if lifetime.is_cancelled() {
                return;
            }
            let value = build_ui_delivery(&target, value);
            let version = {
                let mut previous = output.write().unwrap_or_else(|e| e.into_inner());
                if previous.key == value.key {
                    return;
                }
                *previous = value;
                revision.fetch_add(1, Ordering::AcqRel) + 1
            };
            if !lifetime.is_cancelled() {
                publish.publish(version);
            }
        }),
    );
    let completion = Arc::new(crate::monitor::MonitorCompletion::default());
    let context = UiMonitorContext {
        completion: crate::monitor::MonitorCompletionGuard(completion.clone()),
        monitor,
        watch: watch.clone(),
        lifetime: state.lifetime.clone(),
        guard: state.lifetime.track(),
    };
    world
        .get_resource_mut::<SyncObservations>()
        .unwrap()
        .ui
        .insert(
            entity,
            UiWatchSlot {
                completion,
                manifest_path,
                address,
                watch,
                sync,
            },
        );
    world.send_async_closure(RuntimePlugin::UPDATE, move || run_ui_watch(context));
    waits
}

pub(crate) fn stop_ui_watch(
    world: &mut World,
    entity: core_plugin::Entity,
) -> Vec<agent_core::AgentObservationWait> {
    if let Some(slot) = world
        .get_resource_mut::<SyncObservations>()
        .unwrap()
        .ui
        .remove(&entity)
    {
        let wait = slot.completion.clone().wait();
        drop(slot);
        vec![wait]
    } else {
        vec![]
    }
}
async fn run_ui_watch(context: UiMonitorContext) -> Result<(), DtoAsyncError> {
    let _guard = context.guard;
    let _completion = context.completion;
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
