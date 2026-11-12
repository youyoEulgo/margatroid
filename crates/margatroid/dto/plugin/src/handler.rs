use crate::{projection, types::*};
use address::WorldAddressExt;
use agent_core::{Agent, AgentStartParameters, WorldAgentExt};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use client_core::{Client, ClientDisconnected, ClientFrame, ConnectionId, WorldClientExt};
use config_core::{MargatroidConfig, OutboundTarget};
use core_plugin::{Entity, World};
use futures_util::{
    FutureExt,
    stream::{FuturesUnordered, StreamExt},
};
use margatroid_protocol::*;
use serde_json::{Value, json};
use std::{
    any::Any,
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use storage_core::{AgentStorage, RecordRange};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::{Workspace, WorkspaceDefinitionSnapshot, WorkspaceDefinitions};

type ReplyFuture = Pin<Box<dyn Future<Output = Value> + Send>>;
fn error(code: &str, message: &str) -> ProtocolErrorDto {
    ProtocolErrorDto::new(code, message)
}
trait BoundaryError: Sized + Send + 'static {
    fn boundary(value: ProtocolErrorDto) -> Self;
    fn external(self) -> ProtocolErrorDto;
}
impl BoundaryError for ProtocolErrorDto {
    fn boundary(value: Self) -> Self {
        value
    }
    fn external(self) -> Self {
        self
    }
}
macro_rules! domain_error {
    ($ty:ty,$kind:path) => {
        impl BoundaryError for $ty {
            fn boundary(value: ProtocolErrorDto) -> Self {
                Self::new($kind, value.message)
            }
            fn external(self) -> ProtocolErrorDto {
                error(&format!("{:?}", self.kind), &self.message)
            }
        }
    };
}
domain_error!(
    agent_core::AgentError,
    agent_core::AgentErrorKind::InvalidRequest
);
domain_error!(
    workspace_core::WorkspaceError,
    workspace_core::WorkspaceErrorKind::InvalidRequest
);
domain_error!(mcl_core::MclError, mcl_core::MclErrorKind::SourceInvalid);
impl BoundaryError for storage_core::StorageError {
    fn boundary(value: ProtocolErrorDto) -> Self {
        Self::new(
            storage_core::StorageErrorKind::InvalidRequest,
            value.message,
        )
    }
    fn external(self) -> ProtocolErrorDto {
        error(&format!("{:?}", self.kind), "storage operation failed")
    }
}
impl BoundaryError for client_core::ClientError {
    fn boundary(_: ProtocolErrorDto) -> Self {
        Self::InvalidRequest
    }
    fn external(self) -> ProtocolErrorDto {
        error(&format!("{self:?}"), &self.to_string())
    }
}
pub(crate) fn deliver<R, E>(reply: &PreparedReply<R, E>, result: Result<R, E>) {
    if let Some(sender) = reply.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = sender.send(result);
    }
}
fn reject<T, R, E: BoundaryError>(request: Prepared<T, R, E>, failure: ProtocolErrorDto) {
    deliver(&request.reply, Err(E::boundary(failure)));
}
pub(crate) fn client(world: &World, connection: ConnectionId) -> Result<Entity, ProtocolErrorDto> {
    world
        .client_by_connection(connection)
        .filter(|(_, c)| !c.sender.is_closed())
        .map(|(e, _)| e)
        .ok_or_else(|| error("Unregistered", "register this connection first"))
}
fn target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto> {
    let entity = world
        .entity_by_address(address)
        .map_err(|_| error("TargetMissing", "address is not active"))?;
    Ok(entity)
}
pub(crate) fn agent(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto> {
    let entity = target(world, address)?;
    if world.get_component::<Agent>(entity).is_none() {
        return Err(error("AgentMissing", "Agent is not active"));
    }
    Ok(entity)
}
fn request_id(ticket: &Ticket) -> String {
    format!("dto:{}:{}", ticket.connection.get(), ticket.id)
}
fn frame(kind: &str, id: &str, result: Result<Value, ProtocolErrorDto>) -> Value {
    match result {
        Ok(fields) => {
            let mut v = json!({"type":kind,"id":id,"success":true});
            v.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            v
        }
        Err(e) => json!({"type":kind,"id":id,"success":false,"error":e}),
    }
}
fn request<T: Send + 'static, R: Send + 'static, E: BoundaryError>(
    ticket: Arc<Ticket>,
    route: &str,
    input: T,
    response: &str,
    project: impl FnOnce(R) -> Result<Value, ProtocolErrorDto> + Send + 'static,
) -> (DtoRequest, ReplyFuture) {
    let address = (&input as &dyn Any)
        .downcast_ref::<AgentRecordsQueryDto>()
        .map(|v| v.address.clone())
        .or_else(|| {
            (&input as &dyn Any)
                .downcast_ref::<AgentTargetDto>()
                .map(|v| v.address.clone())
        });
    let (sender, receiver) = oneshot::channel::<Result<R, E>>();
    let payload = Prepared {
        ticket: ticket.clone(),
        input,
        reply: Arc::new(Mutex::new(Some(sender))),
    };
    let dto = DtoRequest {
        route: route.into(),
        object: Arc::new(Mutex::new(Some(Box::new(payload)))),
    };
    let response = response.to_owned();
    let future = async move {
        let result = match receiver.await {
            Ok(Ok(v)) => project(v),
            Ok(Err(e)) => Err(e.external()),
            Err(_) => Err(error("ReplyClosed", "operation response was lost")),
        };
        let mut value = frame(&response, &ticket.id, result);
        if matches!(
            response.as_str(),
            "agent.history.res"
                | "agent.log.res"
                | "agent.mcl.res"
                | "agent.task.res"
                | "agent.ui.res"
        ) && let Some(address) = address
        {
            value["address"] = json!(address);
        }
        value
    }
    .boxed();
    (dto, future)
}
fn minimal<T>(_: T) -> Result<Value, ProtocolErrorDto> {
    Ok(json!({}))
}
fn mcl_result(value: mcl_core::MclValue) -> Result<Value, ProtocolErrorDto> {
    use mcl_core::{MclValue, MessageArray};
    let (kind, value) = match value {
        MclValue::Unit => return minimal(()),
        MclValue::Error(v) => {
            return Err(error(
                "MclFailed",
                v["message"].as_str().unwrap_or("MCL execution failed"),
            ));
        }
        MclValue::Array(MessageArray::Message(v)) => ("messages", json!(v)),
        MclValue::Config(v) => ("config", json!(v)),
        MclValue::BlockSnapshots(v) => ("block_snapshots", v),
        MclValue::Import { kind, alias } => ("import", json!({"kind":kind,"alias":alias})),
        MclValue::Inference {
            message,
            stop_reason,
        } => (
            "inference",
            json!({"message":message,"stop_reason":stop_reason}),
        ),
    };
    Ok(json!({"data":{"kind":kind,"value":value}}))
}
fn clean_internal(value: &mut Value) {
    if let Value::Object(fields) = value {
        for key in [
            "instance",
            "agent_instance",
            "start_id",
            "source_instance",
            "driver_instance",
            "vm_id",
        ] {
            fields.remove(key);
        }
    }
}
fn history_record(v: storage_core::StoredHistoryRecord) -> Value {
    let row = v.record;
    json!({"sequence":v.sequence,"turn":row.turn,"kind":"message","content":row.content,"payload":row.payload,"created_at_ms":row.created_at_ms.unwrap_or(0)})
}
fn mcl_record(v: storage_core::StoredHistoryRecord) -> Value {
    let mut row = json!({"sequence":v.sequence,"turn":v.record.turn,"source":v.record.source.unwrap_or_default(),"content":v.record.content,"payload":v.record.payload,"created_at_ms":v.record.created_at_ms.unwrap_or(0)});
    clean_internal(&mut row["payload"]);
    row
}
fn log_record(v: storage_core::StoredAgentLog) -> Value {
    let mut row = serde_json::to_value(v.record).unwrap();
    row["sequence"] = json!(v.sequence);
    clean_internal(&mut row["context_json"]);
    row
}
pub(crate) fn handle_dto_request(world: &mut World, request: DtoRequest) {
    let Some(object) = request
        .object
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    else {
        return;
    };
    match request.route.as_str() {
        "connection.register" => dispatch::<RegisterClientDto>(world, object),
        "workspace.start" => dispatch::<StartWorkspaceDto>(world, object),
        "workspace.stop" => dispatch::<StopWorkspaceDto>(world, object),
        "workspace.restart" => dispatch::<RestartWorkspaceDto>(world, object),
        "agent.start" => dispatch::<StartAgentDto>(world, object),
        "agent.stop" => dispatch::<StopAgentDto>(world, object),
        "agent.restart" => dispatch::<RestartAgentDto>(world, object),
        "mcl.execute" => dispatch::<MclCommandDto>(world, object),
        "agent.ui.req" => crate::ui::query_agent_ui(world, object),
        "workspace.req" => query::<(), Vec<WorkspaceInfoDto>>(world, object, |w, _, _| {
            Ok(projection::workspaces(w))
        }),
        "agent.req" => {
            query::<(), Vec<AgentStateDto>>(world, object, |w, _, _| Ok(projection::agents(w)))
        }
        "agent.turn.req" => query::<AgentTargetDto, AgentTurnStateDto>(world, object, |w, _, v| {
            projection::turn(w, agent(w, &v.address)?)
                .ok_or_else(|| error("AgentMissing", "Agent runtime is missing"))
        }),
        "agent.task.req" => query::<AgentTargetDto, AgentTaskStateDto>(world, object, |w, _, v| {
            projection::tasks(w, agent(w, &v.address)?)
                .ok_or_else(|| error("AgentMissing", "Agent tasks are missing"))
        }),
        "workspace.reload" => reload(world, object),
        "agent.history.req" | "agent.log.req" | "agent.mcl.req" => {
            records(world, &request.route, object)
        }
        _ => (),
    }
}
trait IntoEvent: Sized + TryFrom<Box<dyn Any + Send>, Error = Box<dyn Any + Send>> {
    type Event: core_plugin::Event;
    fn into_event(self, world: &World) -> Result<Self::Event, ()>;
}
fn dispatch<T: IntoEvent>(world: &mut World, object: Box<dyn Any + Send>) {
    if let Ok(v) = T::try_from(object)
        && let Ok(event) = v.into_event(world)
    {
        world.send_event(event);
    }
}
macro_rules! converter {
    ($name:ident,$input:ty,$out:ty,$err:ty,$event:ty,$body:expr) => {
        struct $name(Prepared<$input, $out, $err>);
        impl TryFrom<Box<dyn Any + Send>> for $name {
            type Error = Box<dyn Any + Send>;
            fn try_from(object: Box<dyn Any + Send>) -> Result<Self, Self::Error> {
                object
                    .downcast::<Prepared<$input, $out, $err>>()
                    .map(|v| Self(*v))
            }
        }
        impl IntoEvent for $name {
            type Event = $event;
            fn into_event(self, world: &World) -> Result<Self::Event, ()> {
                let request = self.0;
                let result: Result<Self::Event, ProtocolErrorDto> = ($body)(world, &request);
                match result {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        reject(request, e);
                        Err(())
                    }
                }
            }
        }
    };
}
converter!(
    RegisterClientDto,
    RegisterConnectionDto,
    client_core::RegisteredClient,
    client_core::ClientError,
    client_core::RegisterClient,
    |world: &World,
     r: &Prepared<
        RegisterConnectionDto,
        client_core::RegisteredClient,
        client_core::ClientError,
    >| {
        let sender = world
            .get_resource::<ConnState>()
            .and_then(|c| c.get(r.ticket.connection))
            .ok_or_else(|| error("ConnectionMissing", "connection closed"))?;
        Ok(client_core::RegisterClient {
            id: request_id(&r.ticket),
            connection_id: r.ticket.connection,
            client_type: r.input.client_type.clone(),
            name: r.input.name.clone(),
            sender,
            reply: r.reply.clone(),
        })
    }
);
converter!(
    StartWorkspaceDto,
    WorkspacePathDto,
    workspace_core::WorkspaceOperationOutput,
    workspace_core::WorkspaceError,
    workspace_core::StartWorkspace,
    |world: &World,
     r: &Prepared<
        WorkspacePathDto,
        workspace_core::WorkspaceOperationOutput,
        workspace_core::WorkspaceError,
    >| {
        client(world, r.ticket.connection)?;
        Ok(workspace_core::StartWorkspace {
            id: request_id(&r.ticket),
            path: r.input.path.clone().into(),
            reply: r.reply.clone(),
        })
    }
);
converter!(
    RestartWorkspaceDto,
    WorkspacePathDto,
    workspace_core::WorkspaceOperationOutput,
    workspace_core::WorkspaceError,
    workspace_core::RestartWorkspace,
    |world: &World,
     r: &Prepared<
        WorkspacePathDto,
        workspace_core::WorkspaceOperationOutput,
        workspace_core::WorkspaceError,
    >| {
        client(world, r.ticket.connection)?;
        Ok(workspace_core::RestartWorkspace {
            id: request_id(&r.ticket),
            path: r.input.path.clone().into(),
            reply: r.reply.clone(),
        })
    }
);
converter!(
    StopWorkspaceDto,
    WorkspacePathDto,
    workspace_core::WorkspaceOperationOutput,
    workspace_core::WorkspaceError,
    workspace_core::StopWorkspace,
    |world: &World,
     r: &Prepared<
        WorkspacePathDto,
        workspace_core::WorkspaceOperationOutput,
        workspace_core::WorkspaceError,
    >| {
        client(world, r.ticket.connection)?;
        let path = PathBuf::from(&r.input.path);
        let entity = world
            .query_with::<Workspace>()
            .result()
            .into_iter()
            .find(|e| {
                world
                    .get_component::<Workspace>(*e)
                    .is_some_and(|w| w.definition_path == path)
            })
            .ok_or_else(|| error("WorkspaceMissing", "workspace is not running"))?;
        Ok(workspace_core::StopWorkspace {
            id: request_id(&r.ticket),
            workspace: entity,
            reply: r.reply.clone(),
        })
    }
);
#[derive(Clone)]
struct AgentInput {
    start: AgentStartDto,
    snapshot: WorkspaceDefinitionSnapshot,
}
fn parameters(
    world: &World,
    input: &AgentInput,
) -> Result<(Entity, String, AgentStartParameters), ProtocolErrorDto> {
    let entity = target(world, &input.start.workspace)?;
    let ws = world
        .get_component::<Workspace>(entity)
        .ok_or_else(|| error("WorkspaceMissing", "Workspace is not active"))?;
    let definition = input
        .snapshot
        .get(&ws.definition_path)
        .ok_or_else(|| error("DefinitionMissing", "Workspace definition is absent"))?;
    let member = definition
        .members
        .iter()
        .find(|m| m.name == input.start.name)
        .ok_or_else(|| error("MemberMissing", "Agent member is absent"))?;
    let base = ws
        .definition_path
        .parent()
        .ok_or_else(|| error("InvalidPath", "Workspace directory is missing"))?;
    let home = if member.home_root.is_absolute() {
        member.home_root.clone()
    } else {
        base.join(&member.home_root)
    };
    let directory = base.join("agents").join(&member.name);
    Ok((
        entity,
        member.name.clone(),
        AgentStartParameters {
            driver: member.driver.clone(),
            project_root: definition.project_root.clone(),
            home_root: home,
            database_path: directory.join("state.sqlite"),
            settings_path: directory.join("setting.toml"),
        },
    ))
}
converter!(
    StartAgentDto,
    AgentInput,
    Entity,
    agent_core::AgentError,
    agent_core::StartAgent,
    |world: &World, r: &Prepared<AgentInput, Entity, agent_core::AgentError>| {
        client(world, r.ticket.connection)?;
        let (workspace, name, parameters) = parameters(world, &r.input)?;
        Ok(agent_core::StartAgent {
            id: request_id(&r.ticket),
            workspace,
            name,
            parameters,
            reply: r.reply.clone(),
        })
    }
);
#[derive(Clone)]
struct RestartInput {
    target: AgentTargetDto,
    snapshot: WorkspaceDefinitionSnapshot,
}
converter!(
    RestartAgentDto,
    RestartInput,
    Entity,
    agent_core::AgentError,
    agent_core::RestartAgent,
    |world: &World, r: &Prepared<RestartInput, Entity, agent_core::AgentError>| {
        client(world, r.ticket.connection)?;
        let entity = agent(world, &r.input.target.address)?;
        let a = world.get_component::<Agent>(entity).unwrap();
        let address = projection::address(world, a.workspace)
            .ok_or_else(|| error("WorkspaceMissing", "Workspace address missing"))?;
        let input = AgentInput {
            start: AgentStartDto {
                workspace: address,
                name: a.name.clone(),
            },
            snapshot: r.input.snapshot.clone(),
        };
        let (workspace, name, parameters) = parameters(world, &input)?;
        Ok(agent_core::RestartAgent {
            id: request_id(&r.ticket),
            workspace,
            name,
            parameters,
            reply: r.reply.clone(),
        })
    }
);
converter!(
    StopAgentDto,
    AgentTargetDto,
    Entity,
    agent_core::AgentError,
    agent_core::StopAgent,
    |world: &World, r: &Prepared<AgentTargetDto, Entity, agent_core::AgentError>| {
        client(world, r.ticket.connection)?;
        let entity = agent(world, &r.input.address)?;
        Ok(agent_core::StopAgent {
            id: request_id(&r.ticket),
            agent: entity,
            reply: r.reply.clone(),
        })
    }
);
converter!(
    MclCommandDto,
    MclExecuteDto,
    mcl_core::MclValue,
    mcl_core::MclError,
    mcl_core::MclCommand,
    |world: &World, r: &Prepared<MclExecuteDto, mcl_core::MclValue, mcl_core::MclError>| {
        let source = client(world, r.ticket.connection)?;
        let target = agent(world, &r.input.address)?;
        let source_address = projection::address(world, source)
            .ok_or_else(|| error("SourceMissing", "Client address is missing"))?;
        Ok(mcl_core::MclCommand {
            id: request_id(&r.ticket),
            source,
            source_address,
            source_instance: None,
            target,
            instruction: mcl_core::MclInstruction {
                text: r.input.command.clone(),
                arguments: r.input.arguments.clone(),
            },
            cancel: CancellationToken::new(),
            reply: r.reply.clone(),
        })
    }
);
fn query<T: Send + 'static, R: Send + 'static>(
    world: &World,
    object: Box<dyn Any + Send>,
    read: impl FnOnce(&World, Entity, &T) -> Result<R, ProtocolErrorDto>,
) {
    if let Ok(r) = object.downcast::<Prepared<T, R>>() {
        let result = client(world, r.ticket.connection).and_then(|e| read(world, e, &r.input));
        deliver(&r.reply, result);
    }
}
fn reload(world: &World, object: Box<dyn Any + Send>) {
    let Ok(r) = object.downcast::<Prepared<(), (), workspace_core::WorkspaceError>>() else {
        return;
    };
    if let Err(e) = client(world, r.ticket.connection) {
        reject(*r, e);
        return;
    }
    let definitions = world
        .get_resource::<WorkspaceDefinitions>()
        .cloned()
        .unwrap_or_default();
    world.send_async_closure(
        app_runtime_plugin::RuntimePlugin::UPDATE,
        move || async move {
            let result =
                tokio::task::spawn_blocking(move || definitions.reload().map(|_| ())).await;
            let result = result.unwrap_or_else(|e| {
                Err(workspace_core::WorkspaceError::new(
                    workspace_core::WorkspaceErrorKind::CleanupFailed,
                    e.to_string(),
                ))
            });
            deliver(&r.reply, result);
            Ok::<_, DtoAsyncError>(())
        },
    );
}
fn records(world: &World, route: &str, object: Box<dyn Any + Send>) {
    macro_rules! page {
        ($record:ty,$function:path) => {{
            let Ok(r) = object.downcast::<Prepared<
                AgentRecordsQueryDto,
                storage_core::RecordPage<$record>,
                storage_core::StorageError,
            >>() else {
                return;
            };
            let prepared = client(world, r.ticket.connection)
                .and_then(|_| agent(world, &r.input.address))
                .and_then(|e| {
                    world
                        .get_component::<AgentStorage>(e)
                        .cloned()
                        .ok_or_else(|| error("StorageMissing", "Agent storage is missing"))
                });
            let storage = match prepared {
                Ok(v) => v,
                Err(e) => {
                    reject(*r, e);
                    return;
                }
            };
            let range = RecordRange {
                before_sequence: r.input.before_sequence,
                after_sequence: r.input.after_sequence,
                limit: r.input.limit.unwrap_or(50),
            };
            world.send_async_closure(
                app_runtime_plugin::RuntimePlugin::UPDATE,
                move || async move {
                    let result = tokio::task::spawn_blocking(move || $function(&storage, range))
                        .await
                        .unwrap_or_else(|e| {
                            Err(storage_core::StorageError::new(
                                storage_core::StorageErrorKind::WorkerFailed,
                                e.to_string(),
                            ))
                        });
                    deliver(&r.reply, result);
                    Ok::<_, DtoAsyncError>(())
                },
            );
        }};
    }
    match route {
        "agent.history.req" => page!(
            storage_core::StoredHistoryRecord,
            storage_core::read_message_page
        ),
        "agent.mcl.req" => page!(
            storage_core::StoredHistoryRecord,
            storage_core::read_mcl_page
        ),
        _ => page!(
            storage_core::StoredAgentLog,
            storage_core::read_agent_log_page
        ),
    }
}
pub(crate) fn handle_abort_control(world: &mut World, event: AbortTurnControl) {
    let Some(r) = event
        .request
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    else {
        return;
    };
    let result = (|| {
        client(world, r.ticket.connection)?;
        let entity = agent(world, &r.input.address)?;
        world
            .agent_abort_turn(entity, &r.input.turn)
            .map_err(|e| error(&format!("{:?}", e.kind), &e.message))?;
        if let Some(active) = world
            .get_component::<inference_core::ActiveInference>(entity)
            .and_then(|a| a.current.as_ref())
        {
            active.cancel();
        }
        if let Some(tasks) = world.get_component::<tool_core::ToolTasks>(entity) {
            tasks.abort_unreturned();
        }
        Ok(())
    })();
    deliver(&r.reply, result);
}
pub(crate) fn handle_disconnect(world: &mut World, connection: ConnectionId) {
    world
        .get_resource_mut::<SyncState>()
        .unwrap()
        .sent
        .retain(|(id, _), _| *id != connection.get());
}
pub(crate) fn handle_agent_exception(world: &mut World, event: agent_core::AgentException) {
    if world.is_alive(event.agent) {
        world
            .get_resource_mut::<SyncState>()
            .unwrap()
            .errors
            .insert(
                event.agent,
                error(&format!("{:?}", event.error.kind), &event.error.message),
            );
    }
}

fn page_fields<T>(
    address: String,
    page: storage_core::RecordPage<T>,
    convert: impl Fn(T) -> Value,
) -> Value {
    json!({"address":address,"records":page.records.into_iter().map(convert).collect::<Vec<_>>(),"next_before_sequence":page.next_before_sequence,"next_after_sequence":page.next_after_sequence})
}
async fn prepare_request(
    state: &DtoState,
    connection: ConnectionId,
    message: ClientMessage,
) -> Result<(Option<DtoRequest>, Option<AbortTurnControl>, ReplyFuture), Value> {
    let route = serde_json::to_value(&message).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_owned();
    let id = message.id().to_owned();
    let response = if route.ends_with(".req") {
        format!("{}res", &route[..route.len() - 3])
    } else {
        format!("{route}.result")
    };
    let ticket = state.connections.claim(connection, &id).ok_or_else(|| {
        frame(
            &response,
            &id,
            Err(error("DuplicateRequest", "request id is already in flight")),
        )
    })?;
    macro_rules! build {
        ($input:expr,$out:ty,$err:ty,$project:expr) => {{
            let (event, waiter) =
                request::<_, $out, $err>(ticket, &route, $input, &response, $project);
            Ok((Some(event), None, waiter))
        }};
    }
    match message {
        ClientMessage::ConnectionRegister { message, .. } => build!(
            message,
            client_core::RegisteredClient,
            client_core::ClientError,
            |v| Ok(json!({"client":{"address":v.address,"client_type":v.client_type}}))
        ),
        ClientMessage::WorkspaceReq { .. } => {
            build!((), Vec<WorkspaceInfoDto>, ProtocolErrorDto, |v| Ok(
                json!({"workspaces":v})
            ))
        }
        ClientMessage::AgentUiReq { message, .. } => {
            let address = message.address.clone();
            build!(
                message,
                Option<Arc<agent_driver_core::DriverUiBundle>>,
                ProtocolErrorDto,
                move |value| {
                    let ui = crate::ui::project_agent_ui(value)?;
                    crate::ui::check_ui_budget(&address, &ui)?;
                    Ok(json!({"address":address,"ui":ui}))
                }
            )
        }
        ClientMessage::AgentReq { .. } => build!((), Vec<AgentStateDto>, ProtocolErrorDto, |v| Ok(
            json!({"agents":v})
        )),
        ClientMessage::WorkspaceStart { message, .. }
        | ClientMessage::WorkspaceStop { message, .. }
        | ClientMessage::WorkspaceRestart { message, .. } => build!(
            message,
            workspace_core::WorkspaceOperationOutput,
            workspace_core::WorkspaceError,
            minimal
        ),
        ClientMessage::WorkspaceReload { .. } => {
            build!((), (), workspace_core::WorkspaceError, minimal)
        }
        ClientMessage::AgentStart { message, .. } => {
            let definitions = state.definitions.clone();
            let snapshot = tokio::task::spawn_blocking(move || definitions.reload())
                .await
                .map_err(|_| {
                    frame(
                        &response,
                        &id,
                        Err(error("ReloadFailed", "definition worker failed")),
                    )
                })?
                .map_err(|e| frame(&response, &id, Err(e.external())))?;
            build!(
                AgentInput {
                    start: message,
                    snapshot
                },
                Entity,
                agent_core::AgentError,
                minimal
            )
        }
        ClientMessage::AgentRestart { message, .. } => {
            let definitions = state.definitions.clone();
            let snapshot = tokio::task::spawn_blocking(move || definitions.reload())
                .await
                .map_err(|_| {
                    frame(
                        &response,
                        &id,
                        Err(error("ReloadFailed", "definition worker failed")),
                    )
                })?
                .map_err(|e| frame(&response, &id, Err(e.external())))?;
            build!(
                RestartInput {
                    target: message,
                    snapshot
                },
                Entity,
                agent_core::AgentError,
                minimal
            )
        }
        ClientMessage::AgentStop { message, .. } => {
            build!(message, Entity, agent_core::AgentError, minimal)
        }
        ClientMessage::MclExecute { message, .. } => {
            build!(message, mcl_core::MclValue, mcl_core::MclError, mcl_result)
        }
        ClientMessage::AgentTurnReq { message, .. } => {
            build!(message, AgentTurnStateDto, ProtocolErrorDto, |v| Ok(
                json!({"state":v})
            ))
        }
        ClientMessage::AgentTaskReq { message, .. } => {
            build!(message, AgentTaskStateDto, ProtocolErrorDto, |v| Ok(
                json!({"address":v.address,"tasks":v.tasks})
            ))
        }
        ClientMessage::AgentTurnAbort { message, .. } => {
            let (sender, receiver) = oneshot::channel();
            let req = Prepared {
                ticket: ticket.clone(),
                input: message,
                reply: Arc::new(Mutex::new(Some(sender))),
            };
            let future = async move {
                let result = receiver
                    .await
                    .unwrap_or_else(|_| Err(error("ReplyClosed", "control response lost")));
                frame(
                    "agent.turn.abort.result",
                    &ticket.id,
                    result.map(|()| json!({})),
                )
            }
            .boxed();
            Ok((
                None,
                Some(AbortTurnControl {
                    request: Arc::new(Mutex::new(Some(req))),
                }),
                future,
            ))
        }
        ClientMessage::AgentHistoryReq { message, .. } => {
            let address = message.address.clone();
            build!(
                message,
                storage_core::RecordPage<storage_core::StoredHistoryRecord>,
                storage_core::StorageError,
                move |v| Ok(page_fields(address, v, history_record))
            )
        }
        ClientMessage::AgentMclReq { message, .. } => {
            let address = message.address.clone();
            build!(
                message,
                storage_core::RecordPage<storage_core::StoredHistoryRecord>,
                storage_core::StorageError,
                move |v| Ok(page_fields(address, v, mcl_record))
            )
        }
        ClientMessage::AgentLogReq { message, .. } => {
            let address = message.address.clone();
            build!(
                message,
                storage_core::RecordPage<storage_core::StoredAgentLog>,
                storage_core::StorageError,
                move |v| Ok(page_fields(address, v, log_record))
            )
        }
    }
}
fn enqueue(
    world: &World,
    connection: ConnectionId,
    sender: &client_core::ClientSender,
    value: Value,
) -> bool {
    let Ok(message) = serde_json::from_value::<ServerMessage>(value) else {
        return false;
    };
    let Ok(text) = encode_server_message(&message) else {
        return false;
    };
    if sender.try_send(ClientFrame::Text(text)).is_ok() {
        true
    } else {
        if let Some(c) = world.get_resource::<ConnState>() {
            c.cancel(connection);
        }
        false
    }
}
pub(crate) fn recipients(
    world: &World,
    targets: &[OutboundTarget],
) -> Vec<(ConnectionId, client_core::ClientSender)> {
    let mut rows = BTreeMap::new();
    for entity in world.query_with::<Client>().result() {
        let Some(c) = world.get_component::<Client>(entity) else {
            continue;
        };
        let matches = targets.iter().any(|target| match target {
            OutboundTarget::Broadcast => true,
            OutboundTarget::Type(kind) => &c.client_type == kind,
            OutboundTarget::Name(name) => &c.name == name,
        });
        if matches && !c.sender.is_closed() {
            rows.insert(c.connection_id, c.sender.clone());
        }
    }
    rows.into_iter().collect()
}
pub(crate) fn handle_outbound(world: &World, event: WebSocketMessageSend) {
    for (c, s) in recipients(world, &[event.target]) {
        enqueue(world, c, &s, event.message.clone());
    }
}
pub(crate) fn publish_state(world: &mut World) {
    let targets = world
        .get_resource::<MargatroidConfig>()
        .map(|c| c.backend_state().to_vec())
        .unwrap_or_default();
    let senders = recipients(world, &targets);
    let workspaces = projection::workspaces(world);
    let mut agents = projection::agents(world);
    for row in &mut agents {
        if let Ok(entity) = agent(world, &row.address) {
            row.error = world
                .get_resource::<SyncState>()
                .unwrap()
                .errors
                .get(&entity)
                .cloned();
        }
    }
    let mut messages = vec![
        (
            "workspace".to_owned(),
            json!({"type":"workspace.sync","workspaces":workspaces}),
        ),
        (
            "agent".to_owned(),
            json!({"type":"agent.sync","agents":agents}),
        ),
    ];
    let entities = world.query_with::<Agent>().result();
    for entity in &entities {
        if let Some(v) = projection::turn(world, *entity) {
            messages.push((
                format!("turn:{}", v.address),
                json!({"type":"agent.turn.sync","address":v.address,"turn":v.turn}),
            ));
        }
        if let Some(v) = projection::tasks(world, *entity) {
            messages.push((
                format!("task:{}", v.address),
                json!({"type":"agent.task.sync","address":v.address,"tasks":v.tasks}),
            ));
        }
    }
    for (connection, sender) in &senders {
        for (key, value) in &messages {
            let same = world
                .get_resource::<SyncState>()
                .unwrap()
                .sent
                .get(&(connection.get(), key.clone()))
                == Some(value);
            if !same && enqueue(world, *connection, sender, value.clone()) {
                world
                    .get_resource_mut::<SyncState>()
                    .unwrap()
                    .sent
                    .insert((connection.get(), key.clone()), value.clone());
            }
        }
    }
    world
        .get_resource_mut::<SyncState>()
        .unwrap()
        .records
        .retain(|entity, state| {
            let keep = entities.contains(entity);
            if !keep {
                state.lock().unwrap().cancel.cancel();
            }
            keep
        });
    for entity in entities {
        let Some(storage) = world.get_component::<AgentStorage>(entity).cloned() else {
            continue;
        };
        let Some(address) = projection::address(world, entity) else {
            continue;
        };
        let position = world
            .get_resource_mut::<SyncState>()
            .unwrap()
            .records
            .entry(entity)
            .or_default()
            .clone();
        {
            let mut state = position.lock().unwrap();
            if state.running {
                continue;
            }
            state.running = true;
        }
        let senders = senders.clone();
        let connections = world.get_resource::<ConnState>().unwrap().clone();
        world.send_async_closure(app_runtime_plugin::RuntimePlugin::UPDATE,move ||async move {
            let result=tokio::task::spawn_blocking(move|| {
                struct RunningGuard(Arc<Mutex<RecordPosition>>);
                impl Drop for RunningGuard {fn drop(&mut self){self.0.lock().unwrap_or_else(|e|e.into_inner()).running=false;}}
                let _guard=RunningGuard(position.clone());
                let current=storage_core::read_record_watermarks(&storage).map_err(|e|DtoAsyncError(e.to_string()))?;
                let (message_position,mcl_position,log_position,cancel)={
                    let mut pos=position.lock().unwrap();
                    if !pos.initialized {pos.message=current.message;pos.mcl=current.mcl;pos.log=current.agent_log;pos.initialized=true;return Ok(());}
                    (pos.message,pos.mcl,pos.log,pos.cancel.clone())
                };
                if cancel.is_cancelled() || senders.is_empty(){return Ok(());}
                let messages=storage_core::read_message_page(&storage,RecordRange{after_sequence:Some(message_position),before_sequence:None,limit:50}).map_err(|e|DtoAsyncError(e.to_string()))?;
                let mcl=storage_core::read_mcl_page(&storage,RecordRange{after_sequence:Some(mcl_position),before_sequence:None,limit:50}).map_err(|e|DtoAsyncError(e.to_string()))?;
                let logs=storage_core::read_agent_log_page(&storage,RecordRange{after_sequence:Some(log_position),before_sequence:None,limit:50}).map_err(|e|DtoAsyncError(e.to_string()))?;
                let mut frames=Vec::new();
                for row in &messages.records {frames.push(json!({"type":"agent.message.full.sync","address":address,"turn":row.record.turn,"generation":row.record.payload.get("request_id").and_then(Value::as_str),"history_sequence":row.sequence,"message":row.record.payload,"error":null}));}
                if !mcl.records.is_empty(){frames.push(json!({"type":"agent.mcl.sync","address":address,"records":mcl.records.iter().cloned().map(mcl_record).collect::<Vec<_>>()}));}
                if !logs.records.is_empty(){frames.push(json!({"type":"agent.log.sync","address":address,"records":logs.records.iter().cloned().map(log_record).collect::<Vec<_>>()}));}
                for value in frames {if cancel.is_cancelled(){return Ok(());}let message:ServerMessage=serde_json::from_value(value).map_err(|e|DtoAsyncError(e.to_string()))?;let text=encode_server_message(&message).map_err(|e|DtoAsyncError(e.to_string()))?;
                    for(connection,sender)in &senders {if sender.try_send(ClientFrame::Text(text.clone())).is_err(){connections.cancel(*connection);}}
                }
                let mut pos=position.lock().unwrap();
                if let Some(row)=messages.records.last(){pos.message=row.sequence;}
                if let Some(row)=mcl.records.last(){pos.mcl=row.sequence;}
                if let Some(row)=logs.records.last(){pos.log=row.sequence;}
                pos.running=false;Ok::<_,DtoAsyncError>(())
            }).await;
            result.map_err(|e|DtoAsyncError(e.to_string()))?
        });
    }
}
pub(crate) async fn ws_handler(
    axum::extract::State(state): axum::extract::State<DtoState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    upgrade: axum::extract::ws::WebSocketUpgrade,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let header = |name| -> Result<Option<String>, ()> {
        headers
            .get(name)
            .map(|value| value.to_str().map(str::to_owned).map_err(|_| ()))
            .transpose()
    };
    let (Ok(origin), Ok(host)) = (header("origin"), header("host")) else {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    };
    if state
        .ingress
        .evaluate(&config_core::IngressRequest { peer, origin, host })
        .is_err()
    {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    if state.events.get().is_none() || state.lifetime.is_cancelled() {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Some((connection, outbound, cancel)) = state.connections.allocate() else {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let guard = state.lifetime.track();
    let reservation = ConnectionReservation {
        state: state.clone(),
        connection,
    };
    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            let _reservation = reservation;
            serve_connection(state, connection, socket, outbound, cancel).await;
        })
}
struct ConnectionReservation {
    state: DtoState,
    connection: ConnectionId,
}
impl Drop for ConnectionReservation {
    fn drop(&mut self) {
        if self.state.connections.remove(self.connection)
            && let Some(events) = self.state.events.get()
        {
            events.send_event(ClientDisconnected {
                connection_id: self.connection,
            });
        }
    }
}

fn encode_response(value: Value) -> Option<String> {
    if let Ok(message) = serde_json::from_value::<ServerMessage>(value.clone())
        && let Ok(text) = encode_server_message(&message)
    {
        return Some(text);
    }
    let kind = value.get("type")?.as_str()?;
    let id = value.get("id")?.as_str()?;
    let mut failure = frame(
        kind,
        id,
        Err(error(
            "ResponseTooLarge",
            "response cannot fit the protocol budget",
        )),
    );
    if let Some(address) = value.get("address") {
        failure["address"] = address.clone();
    }
    serde_json::from_value::<ServerMessage>(failure)
        .ok()
        .and_then(|v| encode_server_message(&v).ok())
}
fn invalid_reply(text: &str) -> Option<Value> {
    let raw: Value = serde_json::from_str(text).ok()?;
    let kind = raw.get("type")?.as_str()?;
    let id = raw.get("id")?.as_str()?;
    if !valid_request_id(id) {
        return None;
    }
    if !matches!(
        kind,
        "connection.register"
            | "workspace.req"
            | "workspace.start"
            | "workspace.stop"
            | "workspace.restart"
            | "workspace.reload"
            | "agent.req"
            | "agent.ui.req"
            | "agent.start"
            | "agent.stop"
            | "agent.restart"
            | "agent.turn.req"
            | "agent.turn.abort"
            | "agent.task.req"
            | "agent.history.req"
            | "agent.log.req"
            | "agent.mcl.req"
            | "mcl.execute"
    ) {
        return None;
    }
    let response = if kind.ends_with(".req") {
        format!("{}res", &kind[..kind.len() - 3])
    } else {
        format!("{kind}.result")
    };
    let mut result = frame(
        &response,
        id,
        Err(error("InvalidRequest", "invalid protocol request")),
    );
    if matches!(
        response.as_str(),
        "agent.task.res" | "agent.history.res" | "agent.log.res" | "agent.mcl.res" | "agent.ui.res"
    ) {
        result["address"] = json!(raw["message"]["address"].as_str().unwrap_or_default());
    }
    Some(result)
}
async fn serve_connection(
    state: DtoState,
    connection: ConnectionId,
    socket: axum::extract::ws::WebSocket,
    mut pushes: tokio::sync::mpsc::Receiver<ClientFrame>,
    cancel: CancellationToken,
) {
    use axum::extract::ws::{CloseFrame, Message};
    use futures_util::SinkExt;
    if state.events.get().is_none() {
        return;
    }
    let (mut writer, mut reader) = socket.split();
    let mut pending = FuturesUnordered::<ReplyFuture>::new();
    loop {
        let output = tokio::select! {
            biased;
            _=state.lifetime.cancelled()=>break,
            _=cancel.cancelled()=>break,
            response=pending.next(),if !pending.is_empty()=>{
                let Some(value)=response else{continue;};
                match encode_response(value).ok_or(()) {
                    Ok(text)=>Some(Message::Text(text.into())),Err(_)=>Some(Message::Close(Some(CloseFrame{code:1007,reason:"invalid response".into()}))),
                }
            },
            frame=pushes.recv()=>match frame {
                Some(ClientFrame::Text(text))=>Some(Message::Text(text.into())),
                Some(ClientFrame::Binary(v))=>Some(Message::Binary(v.into())),
                Some(ClientFrame::Close{code,reason})=>Some(Message::Close(Some(CloseFrame{code,reason:reason.into()}))),None=>break,
            },
            incoming=reader.next()=>{
                match incoming {
                    Some(Ok(Message::Text(text)))=>if pending.len() >= 128 {Some(Message::Close(Some(CloseFrame{code:1008,reason:"too many pending requests".into()})))} else {match parse_client_message(&text) {
                        Ok(message)=>{
                            let state=state.clone();
                            pending.push(async move {
                                match prepare_request(&state,connection,message).await {
                                    Ok((request,abort,future))=>{
                                        let events=state.events.get().unwrap();
                                        if let Some(v)=request{events.send_event(v);}
                                        if let Some(v)=abort{events.send_event(v);}
                                        future.await
                                    },
                                    Err(response)=>response,
                                }
                            }.boxed());
                            None
                        },
                        Err(_)=>{
                            if let Some(value)=invalid_reply(&text){pending.push(async move{value}.boxed());None}
                            else{Some(Message::Close(Some(CloseFrame{code:if text.len()>MAX_MESSAGE_BYTES{1009}else{1007},reason:"invalid protocol request".into()})))}
                        },
                    }},
                    Some(Ok(Message::Ping(v)))=>Some(Message::Pong(v)),
                    Some(Ok(Message::Pong(_)))=>None,
                    Some(Ok(Message::Close(v)))=>Some(Message::Close(v)),
                    _=>break,
                }
            },
        };
        if let Some(output) = output {
            let close = matches!(output, Message::Close(_));
            if !matches!(
                tokio::time::timeout(Duration::from_secs(1), writer.send(output)).await,
                Ok(Ok(()))
            ) {
                break;
            }
            if close {
                break;
            }
        }
    }
    pushes.close();
    let _ = tokio::time::timeout(
        Duration::from_millis(250),
        writer.send(Message::Close(None)),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audit_projection_filters_vm_metadata_and_preserves_user_arguments() {
        let arguments = json!([{"instance":"business-value","source_instance":"user-field"}]);
        let value = mcl_record(storage_core::StoredHistoryRecord {
            sequence: 7,
            record: storage_core::HistoryRecord {
                source: Some("client:webui/test:1".into()),
                turn: None,
                kind: "mcl".into(),
                content: "CONFIG SET ? ?".into(),
                payload: json!({"source_instance":"private-vm","arguments":arguments}),
                created_at_ms: Some(1),
            },
        });
        assert!(value["payload"].get("source_instance").is_none());
        assert_eq!(value["payload"]["arguments"], arguments);
        assert_eq!(value["sequence"], 7);
    }
}
