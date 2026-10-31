use crate::types::*;
use address::{Address, WorldAddressExt};
use agent_core::{Agent, AgentStartParameters, AgentState, WorldAgentExt};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use client_core::{Client, ClientRegistrationResult, WorldClientExt};
use config_core::{MargatroidConfig, OutboundTarget};
use core_plugin::{Entity, World};
use inference_core::{ActiveInference, AgentInferenceConfig, ModelRoutes};
use lua_runtime_core::DriverLuaVm;
use margatroid_protocol::*;
use mcl_core::{MclCommand, MclInstruction, MclQueue};
use sandbox_resource_core::SelectedSandbox;
use serde_json::{Value, json};
use server_plugin::{
    RegisterConnection, WebSocketCloseReason, WebSocketConnectionId, WebSocketConnections,
    WebSocketMessage,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use storage_core::{AgentStorage, StorageOperation, StorageValue};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::{
    RestartWorkspace, StartWorkspace, StopWorkspace, Workspace, WorkspaceDefinitions,
    WorkspaceError, WorkspaceOperationOutput,
};

fn error(kind: &str, message: &str) -> ProtocolErrorDto {
    ProtocolErrorDto::new(kind, message)
}
fn failed(id: String, error: ProtocolErrorDto) -> ServerMessage {
    ServerMessage::RequestFailed { id, error }
}
fn address(world: &World, entity: Entity) -> Option<String> {
    world
        .get_component::<Address>(entity)
        .map(|address| address.as_str().to_owned())
}
fn instance(world: &World, entity: Entity) -> Option<String> {
    world
        .get_component::<DriverLuaVm>(entity)
        .map(|vm| vm.instance.start_id.clone())
}
fn operation_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "dto:{}:{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn connected(world: &World, connection: WebSocketConnectionId) -> bool {
    world
        .get_resource::<WebSocketConnections>()
        .and_then(|connections| connections.get(connection))
        .is_some_and(|sender| !sender.is_closed())
}
fn live_client(world: &World, context: &RequestContext) -> bool {
    connected(world, context.connection)
        && context.client.is_some_and(|client| {
            world
                .client_by_connection(context.connection)
                .is_some_and(|(current, _)| current == client)
        })
}
fn send(world: &World, connection: WebSocketConnectionId, message: &ServerMessage) -> bool {
    let Some(sender) = world
        .get_resource::<WebSocketConnections>()
        .and_then(|connections| connections.get(connection))
        .filter(|sender| !sender.is_closed())
    else {
        return false;
    };
    let encoded = encode_server_message(message).or_else(|failure| {
        let Some(id) = message.id() else {
            return Err(failure);
        };
        encode_server_message(&failed(id.to_owned(), failure.0))
    });
    encoded
        .ok()
        .is_some_and(|text| sender.try_send(WebSocketMessage::Text(text.into())).is_ok())
}
fn close_invalid(world: &World, connection: WebSocketConnectionId, code: u16) {
    if let Some(sender) = world
        .get_resource::<WebSocketConnections>()
        .and_then(|connections| connections.get(connection))
    {
        world.spawn_async_service(async move {
            let _ = sender
                .close(WebSocketCloseReason::new(code, "invalid protocol request"))
                .await;
        });
    }
}
fn track(
    world: &mut World,
    connection: WebSocketConnectionId,
    client: Option<Entity>,
    network_id: String,
    target: Option<LiveTarget>,
    kind: OperationKind,
) -> Result<String, ProtocolErrorDto> {
    let pending = world.get_resource_mut::<PendingRequests>().unwrap();
    if pending
        .entries
        .values()
        .any(|entry| entry.connection == connection && entry.network_id == network_id)
    {
        return Err(error("DuplicateRequest", "request id is already in flight"));
    }
    if pending.entries.len() >= 4096
        || pending
            .entries
            .values()
            .filter(|entry| entry.connection == connection)
            .count()
            >= 128
    {
        return Err(error("TooManyRequests", "too many requests are in flight"));
    }
    let operation_id = operation_id();
    pending.entries.insert(
        operation_id.clone(),
        RequestContext {
            network_id,
            operation_id: operation_id.clone(),
            client,
            connection,
            target,
            kind,
        },
    );
    Ok(operation_id)
}
fn context(world: &World, id: &str) -> Option<RequestContext> {
    world
        .get_resource::<PendingRequests>()?
        .entries
        .get(id)
        .filter(|context| context.operation_id == id)
        .cloned()
}
fn take(world: &mut World, id: &str) -> Option<RequestContext> {
    world
        .get_resource_mut::<PendingRequests>()?
        .entries
        .remove(id)
}
fn mark_dirty(world: &mut World) {
    world
        .get_resource_mut::<BackendStateReportCache>()
        .unwrap()
        .dirty = true;
}
fn workspace_reference(world: &World, entity: Entity) -> Option<WorkspaceReferenceDto> {
    let workspace = world.get_component::<Workspace>(entity)?;
    Some(WorkspaceReferenceDto::Reference {
        id: address(world, entity)?,
        name: workspace.name.clone(),
        project_root: workspace.project_root.to_string_lossy().into_owned(),
    })
}
fn resolve_workspace(world: &World, value: &str) -> Result<Entity, ProtocolErrorDto> {
    let entity = world.entity_by_address(value).map_err(|_| {
        error(
            "WorkspaceMissing",
            "workspace address is not a unique running instance",
        )
    })?;
    if world.get_component::<Workspace>(entity).is_none() {
        return Err(error(
            "WorkspaceMissing",
            "workspace instance is unavailable",
        ));
    }
    Ok(entity)
}
fn target(
    world: &World,
    workspace: &WorkspaceReferenceDto,
    agent: &str,
) -> Result<LiveTarget, ProtocolErrorDto> {
    let workspace = resolve_workspace(world, workspace.address())?;
    let entity = world.entity_by_address(agent).map_err(|_| {
        error(
            "AgentMissing",
            "agent address is not a unique running instance",
        )
    })?;
    if !world
        .get_component::<Agent>(entity)
        .is_some_and(|agent| agent.workspace == workspace)
    {
        return Err(error(
            "InvalidTarget",
            "agent does not belong to the workspace",
        ));
    }
    Ok(LiveTarget {
        workspace: Some(workspace),
        entity,
        instance: instance(world, entity),
        address: address(world, entity).unwrap(),
    })
}
fn live_target(world: &World, target: &LiveTarget) -> bool {
    world.is_alive(target.entity)
        && address(world, target.entity).as_deref() == Some(target.address.as_str())
        && match target.workspace {
            None => world.get_component::<Workspace>(target.entity).is_some(),
            Some(workspace) => {
                world.get_component::<Workspace>(workspace).is_some()
                    && world
                        .get_component::<Agent>(target.entity)
                        .is_some_and(|agent| agent.workspace == workspace)
                    && instance(world, target.entity) == target.instance
            }
        }
}

pub(crate) fn handle_inbound_message(
    world: &mut World,
    connection: WebSocketConnectionId,
    message: WebSocketMessage,
) {
    if !connected(world, connection) {
        return;
    }
    let WebSocketMessage::Text(text) = message else {
        close_invalid(world, connection, 1007);
        return;
    };
    if text.len() > MAX_MESSAGE_BYTES {
        close_invalid(world, connection, 1009);
        return;
    }
    let parsed = match parse_client_message(&text) {
        Ok(message) => message,
        Err(failure) => {
            let id = serde_json::from_str::<Value>(&text).ok().and_then(|value| {
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| valid_request_id(id))
                    .map(str::to_owned)
            });
            if let Some(id) = id {
                send(world, connection, &failed(id, failure.0));
            } else {
                close_invalid(world, connection, 1007);
            }
            return;
        }
    };
    let network_id = parsed.id().to_owned();
    let result = if let ClientMessage::ConnectionRegister { id, message } = parsed {
        let existing = world
            .client_by_connection(connection)
            .map(|(entity, _)| entity);
        track(
            world,
            connection,
            existing,
            id,
            None,
            OperationKind::Registration,
        )
        .map(|operation| {
            world.send_event(RegisterConnection {
                id: operation,
                connection_id: connection,
                client_type: message.client_type,
                name: message.name,
            });
        })
    } else if let Some((client, _)) = world.client_by_connection(connection) {
        route(world, connection, client, parsed)
    } else {
        Err(error("Unregistered", "register this connection first"))
    };
    if let Err(error) = result {
        send(world, connection, &failed(network_id, error));
    }
}

fn route(
    world: &mut World,
    connection: WebSocketConnectionId,
    client: Entity,
    message: ClientMessage,
) -> Result<(), ProtocolErrorDto> {
    if world
        .get_resource::<PendingRequests>()
        .unwrap()
        .entries
        .values()
        .any(|pending| pending.client == Some(client) && pending.network_id == message.id())
    {
        return Err(error("DuplicateRequest", "request id is already in flight"));
    }
    match message {
        ClientMessage::WorkspaceDefinitionsQuery { id, .. } => {
            send(
                world,
                connection,
                &ServerMessage::WorkspaceDefinitions {
                    id,
                    definitions: definitions(world),
                },
            );
        }
        ClientMessage::StateQuery { id, .. } => {
            let operation = track(
                world,
                connection,
                Some(client),
                id,
                None,
                OperationKind::State,
            )?;
            world
                .get_resource_mut::<BackendStateReportCache>()
                .unwrap()
                .state_waiters
                .push(operation);
            mark_dirty(world);
        }
        ClientMessage::WorkspaceStart { id, message } => {
            let operation = track(
                world,
                connection,
                Some(client),
                id,
                None,
                OperationKind::WorkspaceStart,
            )?;
            world.send_event(StartWorkspace {
                id: operation,
                path: message.path.into(),
            });
        }
        ClientMessage::WorkspaceRestart { id, message } => {
            let operation = track(
                world,
                connection,
                Some(client),
                id,
                None,
                OperationKind::WorkspaceRestart,
            )?;
            world.send_event(RestartWorkspace {
                id: operation,
                path: message.path.into(),
            });
        }
        ClientMessage::WorkspaceStop { id, message } => {
            let workspace = resolve_workspace(world, message.workspace.address())?;
            let reference = workspace_reference(world, workspace).unwrap();
            let target = LiveTarget {
                workspace: None,
                entity: workspace,
                instance: None,
                address: reference.address().into(),
            };
            let operation = track(
                world,
                connection,
                Some(client),
                id,
                Some(target),
                OperationKind::WorkspaceStop { reference },
            )?;
            world.send_event(StopWorkspace {
                id: operation,
                workspace,
            });
        }
        ClientMessage::AgentTurnAbort { id, message } => {
            let live = target(world, &message.workspace, &message.agent)?;
            let workspace = live.workspace.unwrap();
            let operation = track(
                world,
                connection,
                Some(client),
                id,
                Some(live.clone()),
                OperationKind::Abort {
                    workspace: workspace_reference(world, workspace).unwrap(),
                    agent: live.address.clone(),
                    turn: message.turn_id.clone(),
                },
            )?;
            world.send_event(AbortTurnControl {
                operation_id: operation,
                agent: live.entity,
                workspace,
                instance: message.agent_instance,
                turn: message.turn_id,
            });
        }
        ClientMessage::MclCommand { id, message } => {
            let live = target(world, &message.workspace, &message.agent)?;
            if world.get_component::<Agent>(live.entity).unwrap().state == AgentState::Working
                && is_user_injection(&message.command, &message.arguments)
            {
                return Err(error(
                    "InvalidRequest",
                    "cannot inject a user message while the Agent is working",
                ));
            }
            let source = address(world, client)
                .ok_or_else(|| error("Unregistered", "client address is unavailable"))?;
            let operation = track(
                world,
                connection,
                Some(client),
                id,
                Some(live.clone()),
                OperationKind::Mcl {
                    source,
                    target: live.address.clone(),
                },
            )?;
            let (sender, receiver) = oneshot::channel();
            world.send_event(MclCommand {
                id: operation.clone(),
                source: client,
                source_instance: None,
                target: live.entity,
                instruction: MclInstruction {
                    text: message.command,
                    arguments: message.arguments,
                },
                cancel: CancellationToken::new(),
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            let events = world.event_sender();
            world.spawn_async_service(async move {
                let result = receiver
                    .await
                    .map_err(|_| error("ReplyClosed", "MCL reply was lost"))
                    .and_then(|result| {
                        result.map_err(|failure| {
                            error(&format!("{:?}", failure.kind), "MCL command failed")
                        })
                    });
                events.send_event(ResponseReady {
                    operation_id: operation,
                    payload: AsyncPayload::Mcl(result),
                });
            });
        }
        ClientMessage::HistoryQuery { id, message } => {
            let live = target(world, &message.workspace, &message.agent)?;
            let storage = world
                .get_component::<AgentStorage>(live.entity)
                .cloned()
                .ok_or_else(|| error("StorageMissing", "Agent storage is unavailable"))?;
            let reference = workspace_reference(world, live.workspace.unwrap()).unwrap();
            let agent = live.address.clone();
            let operation = track(
                world,
                connection,
                Some(client),
                id,
                Some(live),
                OperationKind::History,
            )?;
            let events = world.event_sender();
            world.spawn_async_service(async move {
                let result = read_history(storage, reference, agent).await;
                events.send_event(ResponseReady {
                    operation_id: operation,
                    payload: AsyncPayload::History(result),
                });
            });
        }
        ClientMessage::AgentStart { id, message } => {
            agent_operation(world, connection, client, id, message, false)?
        }
        ClientMessage::AgentRestart { id, message } => {
            agent_operation(world, connection, client, id, message, true)?
        }
        ClientMessage::ConnectionRegister { .. } => unreachable!(),
    }
    Ok(())
}

fn agent_operation(
    world: &mut World,
    connection: WebSocketConnectionId,
    client: Entity,
    id: String,
    message: AgentStartDto,
    restarting: bool,
) -> Result<(), ProtocolErrorDto> {
    let workspace = resolve_workspace(world, &message.workspace)?;
    let component = world.get_component::<Workspace>(workspace).unwrap();
    let base = component.definition_path.parent().unwrap_or(Path::new("."));
    let home = PathBuf::from(message.parameters.home_root);
    let home = if home.is_absolute() {
        home
    } else {
        base.join(home)
    };
    let parameters = AgentStartParameters {
        driver: message.parameters.driver,
        project_root: component.project_root.clone(),
        home_root: home.clone(),
        database_path: home.join("state.sqlite"),
        settings_path: home.join("settings.toml"),
    };
    let kind = if restarting {
        OperationKind::AgentRestart {
            workspace,
            name: message.name.clone(),
        }
    } else {
        OperationKind::AgentStart {
            workspace,
            name: message.name.clone(),
        }
    };
    let live = if restarting {
        let agents = world
            .query_with::<Agent>()
            .result()
            .into_iter()
            .filter(|entity| {
                world
                    .get_component::<Agent>(*entity)
                    .is_some_and(|agent| agent.workspace == workspace && agent.name == message.name)
            })
            .collect::<Vec<_>>();
        let [agent] = agents.as_slice() else {
            return Err(error(
                "AgentMissing",
                "restart requires one existing Agent member",
            ));
        };
        LiveTarget {
            workspace: Some(workspace),
            entity: *agent,
            instance: instance(world, *agent),
            address: address(world, *agent)
                .ok_or_else(|| error("AgentMissing", "Agent has no public address"))?,
        }
    } else {
        LiveTarget {
            workspace: None,
            entity: workspace,
            instance: None,
            address: address(world, workspace).unwrap(),
        }
    };
    let operation = track(world, connection, Some(client), id, Some(live), kind)?;
    let (sender, receiver) = oneshot::channel();
    let reply = Arc::new(Mutex::new(Some(sender)));
    if restarting {
        world.send_event(agent_core::RestartAgent {
            id: operation.clone(),
            workspace,
            name: message.name,
            parameters,
            reply,
        });
    } else {
        world.send_event(agent_core::StartAgent {
            id: operation.clone(),
            workspace,
            name: message.name,
            parameters,
            reply,
        });
    }
    let events = world.event_sender();
    world.spawn_async_service(async move {
        let result = receiver
            .await
            .map_err(|_| error("ReplyClosed", "Agent operation reply was lost"))
            .and_then(|result| {
                result.map_err(|failure| {
                    error(&format!("{:?}", failure.kind), "Agent operation failed")
                })
            });
        events.send_event(ResponseReady {
            operation_id: operation,
            payload: AsyncPayload::Agent(result),
        });
    });
    Ok(())
}

fn is_user_injection(command: &str, arguments: &[Value]) -> bool {
    fn user(value: &Value) -> bool {
        match value {
            Value::Array(values) => values.iter().any(user),
            Value::Object(value) => {
                value.get("type").and_then(Value::as_str) == Some("user")
                    || value.get("messages").is_some_and(user)
            }
            _ => false,
        }
    }
    let words = command.split_whitespace().collect::<Vec<_>>();
    words.len() == 4
        && words[0] == "INJECT"
        && words[1] == "?"
        && words[2] == "TO"
        && arguments.iter().any(user)
}

pub(crate) fn handle_disconnect(world: &mut World, connection: WebSocketConnectionId) {
    world
        .get_resource_mut::<PendingRequests>()
        .unwrap()
        .entries
        .retain(|_, pending| pending.connection != connection);
    world
        .get_resource_mut::<BackendStateReportCache>()
        .unwrap()
        .recipients
        .remove(&connection.get());
    world
        .get_resource_mut::<BackendStateReportCache>()
        .unwrap()
        .registered
        .remove(&connection.get());
}

pub(crate) fn handle_registration(world: &mut World, event: ClientRegistrationResult) {
    let Some(context) = context(world, &event.id) else {
        return;
    };
    if !matches!(context.kind, OperationKind::Registration)
        || event.connection_id != context.connection
    {
        return;
    }
    take(world, &event.id);
    if !connected(world, context.connection) {
        return;
    }
    let message = match event.result {
        Ok(result) => {
            let Some((entity, client)) = world.client_by_connection(context.connection) else {
                return;
            };
            if entity != result.client {
                return;
            }
            let Some(resource_id) = address(world, entity) else {
                return;
            };
            ServerMessage::ConnectionRegistered {
                id: context.network_id,
                client: ClientInfoDto {
                    resource_id,
                    client_type: client.client_type.clone(),
                    name: client.name.clone(),
                },
            }
        }
        Err(_) => ServerMessage::ConnectionRegisterFailed {
            id: context.network_id,
            error: error(
                "ClientRegistrationFailed",
                "connection registration was rejected",
            ),
        },
    };
    if send(world, context.connection, &message)
        && matches!(message, ServerMessage::ConnectionRegistered { .. })
        && let Some((client, _)) = world.client_by_connection(context.connection)
    {
        world
            .get_resource_mut::<BackendStateReportCache>()
            .unwrap()
            .registered
            .insert(context.connection.get(), client);
    }
}

pub(crate) fn handle_response(world: &mut World, ready: ResponseReady) {
    let Some(context) = context(world, &ready.operation_id) else {
        return;
    };
    let matches = matches!(
        (&context.kind, &ready.payload),
        (OperationKind::Mcl { .. }, AsyncPayload::Mcl(_))
            | (OperationKind::History, AsyncPayload::History(_))
            | (
                OperationKind::AgentStart { .. } | OperationKind::AgentRestart { .. },
                AsyncPayload::Agent(_)
            )
    );
    if !matches {
        return;
    }
    take(world, &ready.operation_id);
    if !live_client(world, &context) {
        return;
    }
    let target_changed = context.target.as_ref().is_some_and(|target| {
        if matches!(context.kind, OperationKind::AgentRestart { .. }) {
            address(world, target.entity).as_deref() != Some(target.address.as_str())
                || !target.workspace.is_some_and(|workspace| world.get_component::<Workspace>(workspace).is_some() && world.get_component::<Agent>(target.entity).is_some_and(|agent| agent.workspace == workspace))
                || matches!(&ready.payload, AsyncPayload::Agent(Ok(entity)) if *entity != target.entity)
        } else { !live_target(world, target) }
    });
    if target_changed {
        send(
            world,
            context.connection,
            &failed(
                context.network_id,
                error("TargetChanged", "target instance changed before completion"),
            ),
        );
        return;
    }
    let message = match (context.kind, ready.payload) {
        (OperationKind::Mcl { source, target }, AsyncPayload::Mcl(result)) => {
            mark_dirty(world);
            let result = match result {
                Ok(value) => json!({"Ok": value}),
                Err(error) => json!({"Err": error}),
            };
            ServerMessage::MclCommandResult {
                id: context.network_id,
                source,
                target,
                result,
                history_error: None,
            }
        }
        (OperationKind::History, AsyncPayload::History(result)) => match result {
            Ok(history) => {
                merge_history(world, &history);
                ServerMessage::HistoryResult {
                    id: context.network_id,
                    history,
                }
            }
            Err(error) => failed(context.network_id, error),
        },
        (OperationKind::AgentStart { workspace, name }, AsyncPayload::Agent(result)) => {
            agent_response(world, context.network_id, workspace, &name, result, false)
        }
        (OperationKind::AgentRestart { workspace, name }, AsyncPayload::Agent(result)) => {
            agent_response(world, context.network_id, workspace, &name, result, true)
        }
        _ => return,
    };
    send(world, context.connection, &message);
}

fn agent_response(
    world: &mut World,
    id: String,
    workspace: Entity,
    name: &str,
    result: Result<Entity, ProtocolErrorDto>,
    restarting: bool,
) -> ServerMessage {
    match result {
        Ok(entity)
            if world
                .get_component::<Agent>(entity)
                .is_some_and(|agent| agent.workspace == workspace && agent.name == name) =>
        {
            mark_dirty(world);
            match address(world, entity) {
                Some(agent) if restarting => ServerMessage::AgentRestarted { id, agent },
                Some(agent) => ServerMessage::AgentStarted { id, agent },
                None => failed(
                    id,
                    error("AgentMissing", "completed Agent has no public address"),
                ),
            }
        }
        Ok(_) => failed(
            id,
            error("TargetChanged", "Agent operation returned another target"),
        ),
        Err(error) => failed(id, error),
    }
}

pub(crate) fn handle_abort_control(world: &mut World, request: AbortTurnControl) {
    let Some(context) = context(world, &request.operation_id) else {
        return;
    };
    if !matches!(context.kind, OperationKind::Abort { .. }) {
        return;
    }
    take(world, &request.operation_id);
    if !live_client(world, &context) {
        return;
    }
    let valid = context.target.as_ref().is_some_and(|target| {
        target.entity == request.agent
            && target.workspace == Some(request.workspace)
            && live_target(world, target)
    }) && world
        .get_component::<DriverLuaVm>(request.agent)
        .is_some_and(|vm| {
            vm.instance.owner == request.agent && vm.instance.start_id == request.instance
        })
        && world
            .get_component::<Agent>(request.agent)
            .is_some_and(|agent| {
                agent.workspace == request.workspace
                    && agent.turn.as_deref() == Some(request.turn.as_str())
                    && agent.state == AgentState::Working
            });
    let result = if valid {
        world
            .agent_abort_turn(request.agent, &request.turn)
            .map_err(|_| error("AbortRejected", "turn could not be aborted"))
            .map(|()| {
                if let Some(current) = world
                    .get_component::<ActiveInference>(request.agent)
                    .and_then(|active| active.current.as_ref())
                    .filter(|handle| handle.turn == request.turn)
                {
                    current.cancel();
                }
            })
    } else {
        Err(error(
            "InstanceMismatch",
            "Agent instance or turn changed before abort control",
        ))
    };
    if result.is_ok() {
        mark_dirty(world);
    }
    let OperationKind::Abort {
        workspace,
        agent,
        turn,
    } = context.kind
    else {
        return;
    };
    let result = match result {
        Ok(()) => json!({"Ok": null}),
        Err(error) => json!({"Err": error}),
    };
    send(
        world,
        context.connection,
        &ServerMessage::AgentTurnAbortResult {
            id: context.network_id,
            workspace,
            agent,
            turn_id: turn,
            result,
        },
    );
}

pub(crate) fn handle_workspace_result(
    world: &mut World,
    id: String,
    result: Result<WorkspaceOperationOutput, WorkspaceError>,
    operation: &str,
) {
    let Some(context) = context(world, &id) else {
        return;
    };
    if !matches!(
        (&context.kind, operation),
        (OperationKind::WorkspaceStart, "start")
            | (OperationKind::WorkspaceRestart, "restart")
            | (OperationKind::WorkspaceStop { .. }, "stop")
    ) {
        return;
    }
    take(world, &id);
    if !live_client(world, &context) {
        return;
    }
    let message = match result {
        Ok(output) => {
            if let OperationKind::WorkspaceStop { reference } = context.kind {
                if context
                    .target
                    .as_ref()
                    .is_none_or(|target| target.entity != output.workspace)
                {
                    send(
                        world,
                        context.connection,
                        &failed(
                            context.network_id,
                            error("TargetChanged", "workspace stop returned another instance"),
                        ),
                    );
                    return;
                }
                mark_dirty(world);
                ServerMessage::WorkspaceStopped {
                    id: context.network_id,
                    workspace: reference,
                }
            } else {
                let failures = output
                    .member_errors
                    .iter()
                    .map(|diagnostic| WorkspaceMemberFailureDto {
                        agent: diagnostic.name.clone(),
                        driver: world
                            .get_component::<Workspace>(output.workspace)
                            .and_then(|workspace| {
                                world.get_resource::<WorkspaceDefinitions>().and_then(
                                    |definitions| definitions.get(&workspace.definition_path),
                                )
                            })
                            .and_then(|definition| {
                                definition
                                    .members
                                    .into_iter()
                                    .find(|member| member.name == diagnostic.name)
                            })
                            .map(|member| member.driver)
                            .unwrap_or_default(),
                        error: "member startup failed".into(),
                    })
                    .collect();
                world
                    .get_resource_mut::<BackendStateReportCache>()
                    .unwrap()
                    .failures
                    .insert(output.workspace, failures);
                mark_dirty(world);
                match workspace_info(world, output.workspace) {
                    Some(workspace) if operation == "restart" => {
                        ServerMessage::WorkspaceRestarted {
                            id: context.network_id,
                            workspace,
                        }
                    }
                    Some(workspace) => ServerMessage::WorkspaceStarted {
                        id: context.network_id,
                        workspace,
                    },
                    None => failed(
                        context.network_id,
                        error(
                            "WorkspaceMissing",
                            "operation completed without a live public workspace",
                        ),
                    ),
                }
            }
        }
        Err(failure) => {
            let error = error(&format!("{:?}", failure.kind), "workspace operation failed");
            match operation {
                "start" => ServerMessage::WorkspaceStartFailed {
                    id: context.network_id,
                    error,
                },
                "restart" => ServerMessage::WorkspaceRestartFailed {
                    id: context.network_id,
                    error,
                },
                _ => ServerMessage::WorkspaceStopFailed {
                    id: context.network_id,
                    error,
                },
            }
        }
    };
    send(world, context.connection, &message);
}

fn definitions(world: &World) -> Vec<WorkspaceConfigDto> {
    world
        .get_resource::<WorkspaceDefinitions>()
        .map(|definitions| {
            definitions
                .list()
                .into_iter()
                .map(|(path, definition)| WorkspaceConfigDto {
                    path: path.to_string_lossy().into_owned(),
                    cache: WorkspaceDefinitionDto {
                        name: definition.name,
                        project_root: definition.project_root.to_string_lossy().into_owned(),
                        agents: definition
                            .members
                            .into_iter()
                            .map(|member| WorkspaceAgentDefinitionDto {
                                name: member.name,
                                driver: member.driver,
                                home_root: member.home_root.to_string_lossy().into_owned(),
                            })
                            .collect(),
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}
fn workspace_info(world: &World, entity: Entity) -> Option<WorkspaceInfoDto> {
    let workspace = world.get_component::<Workspace>(entity)?;
    Some(WorkspaceInfoDto {
        id: address(world, entity)?,
        name: workspace.name.clone(),
        project_root: workspace.project_root.to_string_lossy().into_owned(),
        definition_path: workspace.definition_path.to_string_lossy().into_owned(),
        status: format!("{:?}", workspace.state).to_lowercase(),
        agents: workspace
            .agents
            .iter()
            .filter_map(|entity| address(world, *entity))
            .collect(),
        member_failures: world
            .get_resource::<BackendStateReportCache>()
            .and_then(|cache| cache.failures.get(&entity))
            .cloned()
            .unwrap_or_default(),
    })
}
fn metadata(world: &World) -> BackendStateDto {
    let workspaces = world
        .query_with::<Workspace>()
        .result()
        .into_iter()
        .filter_map(|entity| workspace_info(world, entity))
        .collect();
    let agents = world
        .query_with::<Agent>()
        .result()
        .into_iter()
        .filter_map(|entity| {
            let agent = world.get_component::<Agent>(entity)?;
            let queue = world.get_component::<MclQueue>(entity);
            let context_window_tokens = world
                .get_component::<AgentInferenceConfig>(entity)
                .and_then(|config| {
                    world
                        .get_resource::<ModelRoutes>()
                        .and_then(|routes| routes.get(&config.parameters.model))
                        .map(|route| route.context_window_tokens)
                });
            Some(AgentStateDto {
                workspace: workspace_reference(world, agent.workspace)?,
                agent: address(world, entity)?,
                agent_instance: instance(world, entity).unwrap_or_default(),
                status: format!("{:?}", agent.state).to_lowercase(),
                turn: agent.turn.clone(),
                sandbox: world
                    .get_component::<SelectedSandbox>(entity)
                    .and_then(|selected| selected.alias.clone()),
                queued_commands: queue.map(|queue| queue.commands.len()).unwrap_or(0),
                pending_command: queue.and_then(|queue| queue.pending.clone()),
                context_window_tokens,
                error: world
                    .get_resource::<BackendStateReportCache>()
                    .and_then(|cache| cache.errors.get(&entity))
                    .cloned(),
            })
        })
        .collect();
    BackendStateDto {
        workspace_definitions: definitions(world),
        workspaces,
        agents,
        histories: Vec::new(),
    }
}

async fn read_history(
    storage: AgentStorage,
    workspace: WorkspaceReferenceDto,
    agent: String,
) -> Result<AgentHistoryDto, ProtocolErrorDto> {
    tokio::task::spawn_blocking(move || history_sync(&storage, workspace, agent))
        .await
        .map_err(|_| error("HistoryReadFailed", "history worker did not complete"))?
}
fn history_sync(
    storage: &AgentStorage,
    workspace: WorkspaceReferenceDto,
    agent: String,
) -> Result<AgentHistoryDto, ProtocolErrorDto> {
    let value = storage_core::execute_storage(storage, StorageOperation::ReadHistoryEntries)
        .map_err(|_| error("HistoryReadFailed", "persisted history could not be read"))?;
    let StorageValue::HistoryEntries(entries) = value else {
        return Err(error(
            "HistoryReadFailed",
            "unexpected history storage response",
        ));
    };
    let messages = entries
        .into_iter()
        .map(|entry| {
            let record = entry.record;
            Ok(HistoryMessageDto {
                key: record.key,
                sequence: entry.sequence,
                turn: record.turn,
                kind: record.kind,
                content: record.content,
                payload: record.payload,
                created_at_ms: record.created_at_ms.ok_or_else(|| {
                    error(
                        "HistoryReadFailed",
                        "persisted history timestamp is missing",
                    )
                })?,
            })
        })
        .collect::<Result<_, ProtocolErrorDto>>()?;
    Ok(AgentHistoryDto {
        workspace,
        agent,
        messages,
    })
}
fn merge_history(world: &mut World, history: &AgentHistoryDto) {
    let cache = world.get_resource_mut::<BackendStateReportCache>().unwrap();
    if let Some(state) = &mut cache.state {
        let previous = state.histories.iter_mut().find(|entry| {
            entry.agent == history.agent && entry.workspace.address() == history.workspace.address()
        });
        match previous {
            Some(previous) if previous != history => {
                *previous = history.clone();
                cache.version += 1;
            }
            None => {
                state.histories.push(history.clone());
                cache.version += 1;
            }
            _ => (),
        }
    }
}

pub(crate) fn handle_snapshot(world: &mut World, ready: SnapshotReady) {
    if world
        .get_resource::<BackendStateReportCache>()
        .unwrap()
        .pending
        != Some(ready.token)
    {
        return;
    }
    world
        .get_resource_mut::<BackendStateReportCache>()
        .unwrap()
        .pending = None;
    let result = ready.result.and_then(|histories| {
        if histories
            .iter()
            .any(|(target, _)| !live_target(world, target))
        {
            return Err(error("TargetChanged", "snapshot target changed"));
        }
        let mut state = metadata(world);
        state.histories = histories.into_iter().map(|(_, history)| history).collect();
        Ok(state)
    });
    let waiters = std::mem::take(
        &mut world
            .get_resource_mut::<BackendStateReportCache>()
            .unwrap()
            .state_waiters,
    );
    match result {
        Ok(state) => {
            let cache = world.get_resource_mut::<BackendStateReportCache>().unwrap();
            if cache.state.as_ref() != Some(&state) {
                cache.version += 1;
            }
            cache.state = Some(state.clone());
            cache.last_refresh = Some(Instant::now());
            for operation in waiters {
                if let Some(context) =
                    take(world, &operation).filter(|context| live_client(world, context))
                {
                    let message = ServerMessage::StateSync {
                        state: state.clone(),
                    };
                    if let Err(failure) = encode_server_message(&message) {
                        send(
                            world,
                            context.connection,
                            &failed(context.network_id, failure.0),
                        );
                    } else {
                        send(world, context.connection, &message);
                    }
                }
            }
        }
        Err(failure) => {
            let cache = world.get_resource_mut::<BackendStateReportCache>().unwrap();
            cache.dirty = true;
            cache.last_refresh = Some(Instant::now());
            cache.retry_after = Some(Instant::now() + Duration::from_secs(1));
            for operation in waiters {
                if let Some(context) =
                    take(world, &operation).filter(|context| live_client(world, context))
                {
                    send(
                        world,
                        context.connection,
                        &failed(context.network_id, failure.clone()),
                    );
                }
            }
        }
    }
}

fn recipients(
    world: &World,
    targets: &[OutboundTarget],
) -> BTreeMap<u64, (WebSocketConnectionId, Entity)> {
    world
        .query_with::<Client>()
        .result()
        .into_iter()
        .filter_map(|entity| {
            let client = world.get_component::<Client>(entity)?;
            let matches = targets.iter().any(|target| match target {
                OutboundTarget::Broadcast => true,
                OutboundTarget::Type(kind) => kind == &client.client_type,
                OutboundTarget::Name(name) => name == &client.name,
            });
            let acknowledged =
                world
                    .get_resource::<BackendStateReportCache>()
                    .is_some_and(|cache| {
                        cache.registered.get(&client.connection_id.get()) == Some(&entity)
                    });
            (matches && acknowledged && connected(world, client.connection_id))
                .then_some((client.connection_id.get(), (client.connection_id, entity)))
        })
        .collect()
}

pub(crate) fn publish_state(world: &mut World) {
    let current = metadata(world);
    let refresh = {
        let cache = world.get_resource_mut::<BackendStateReportCache>().unwrap();
        if cache.metadata.as_ref() != Some(&current) {
            cache.metadata = Some(current);
            cache.dirty = true;
        }
        cache.pending.is_none()
            && cache
                .retry_after
                .is_none_or(|retry| Instant::now() >= retry)
            && (cache.dirty
                || cache.state.is_none()
                || cache
                    .last_refresh
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(1)))
    };
    if refresh {
        let reads = world
            .query_with::<Agent>()
            .result()
            .into_iter()
            .filter_map(|entity| {
                let agent = world.get_component::<Agent>(entity)?;
                let address = address(world, entity)?;
                let reference = workspace_reference(world, agent.workspace)?;
                let storage = world.get_component::<AgentStorage>(entity)?.clone();
                Some((
                    LiveTarget {
                        workspace: Some(agent.workspace),
                        entity,
                        instance: instance(world, entity),
                        address: address.clone(),
                    },
                    storage,
                    reference,
                    address,
                ))
            })
            .collect::<Vec<_>>();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let token = NEXT.fetch_add(1, Ordering::Relaxed);
        let cache = world.get_resource_mut::<BackendStateReportCache>().unwrap();
        cache.pending = Some(token);
        cache.dirty = false;
        let events = world.event_sender();
        world.spawn_async_service(async move {
            let result = tokio::task::spawn_blocking(move || {
                reads
                    .into_iter()
                    .map(|(target, storage, workspace, agent)| {
                        Ok((target, history_sync(&storage, workspace, agent)?))
                    })
                    .collect::<Result<Vec<_>, ProtocolErrorDto>>()
            })
            .await
            .unwrap_or_else(|_| {
                Err(error(
                    "HistoryReadFailed",
                    "snapshot worker did not complete",
                ))
            });
            events.send_event(SnapshotReady { token, result });
        });
    }
    let targets = world
        .get_resource::<MargatroidConfig>()
        .unwrap()
        .backend_state()
        .to_vec();
    let recipients = recipients(world, &targets);
    let cache = world.get_resource::<BackendStateReportCache>().unwrap();
    let version = cache.version;
    let failed_version = cache.failed_state_version;
    let pending = recipients
        .values()
        .filter(|(connection, entity)| {
            cache.recipients.get(&connection.get()) != Some(&(*entity, version))
        })
        .copied()
        .collect::<Vec<_>>();
    world
        .get_resource_mut::<BackendStateReportCache>()
        .unwrap()
        .recipients
        .retain(|id, _| recipients.contains_key(id));
    if pending.is_empty() || failed_version == Some(version) {
        return;
    }
    let Some(state) = world
        .get_resource::<BackendStateReportCache>()
        .unwrap()
        .state
        .clone()
    else {
        return;
    };
    let encoded = match encode_server_message(&ServerMessage::StateSync { state }) {
        Ok(encoded) => WebSocketMessage::Text(encoded.into()),
        Err(_) => {
            world
                .get_resource_mut::<BackendStateReportCache>()
                .unwrap()
                .failed_state_version = Some(version);
            tracing::warn!("backend state exceeds the protocol response budget");
            return;
        }
    };
    for (connection, entity) in pending {
        if world
            .get_resource::<WebSocketConnections>()
            .and_then(|connections| connections.get(connection))
            .is_some_and(|sender| sender.try_send(encoded.clone()).is_ok())
        {
            world
                .get_resource_mut::<BackendStateReportCache>()
                .unwrap()
                .recipients
                .insert(connection.get(), (entity, version));
        }
    }
}

pub(crate) fn handle_outbound(world: &World, outgoing: WebSocketMessageSend) {
    let requested = recipients(world, &[outgoing.target]);
    let configured = match outgoing.message.get("type").and_then(Value::as_str) {
        Some("log") => Some(world.get_resource::<MargatroidConfig>().unwrap().logs()),
        Some("state.sync") => Some(
            world
                .get_resource::<MargatroidConfig>()
                .unwrap()
                .backend_state(),
        ),
        _ => None,
    };
    let allowed = configured.map(|targets| recipients(world, targets));
    let Ok(text) = serde_json::to_string(&outgoing.message) else {
        return;
    };
    if text.len() > MAX_MESSAGE_BYTES {
        return;
    }
    for (id, (connection, _)) in requested {
        if allowed
            .as_ref()
            .is_none_or(|allowed| allowed.contains_key(&id))
            && let Some(sender) = world
                .get_resource::<WebSocketConnections>()
                .and_then(|connections| connections.get(connection))
        {
            let _ = sender.try_send(WebSocketMessage::Text(text.clone().into()));
        }
    }
}
pub(crate) fn handle_log(world: &World, event: LogReady) {
    let targets = world.get_resource::<MargatroidConfig>().unwrap().logs();
    let message = ServerMessage::Log { record: event.0 };
    for (_, (connection, _)) in recipients(world, targets) {
        send(world, connection, &message);
    }
}
pub(crate) fn handle_transport(_world: &mut World, event: TransportEvent) {
    match event {
        TransportEvent::Started(address) => tracing::info!(%address, "WebSocket server started"),
        TransportEvent::Failed => tracing::error!("WebSocket server failed"),
        TransportEvent::Stopped => tracing::info!("WebSocket server stopped"),
        TransportEvent::Connected(connection) => {
            tracing::info!(connection = connection.get(), "WebSocket connected")
        }
        TransportEvent::ProtocolFailed(connection) => {
            tracing::warn!(connection = connection.get(), "WebSocket protocol failed")
        }
    }
}

pub(crate) fn handle_agent_exception(world: &mut World, event: agent_core::AgentException) {
    if world.get_component::<Agent>(event.agent).is_some() {
        world
            .get_resource_mut::<BackendStateReportCache>()
            .unwrap()
            .errors
            .insert(
                event.agent,
                error(&format!("{:?}", event.error.kind), "Agent execution failed"),
            );
        mark_dirty(world);
    }
}
