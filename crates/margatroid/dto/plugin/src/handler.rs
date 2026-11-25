use crate::{projection, types::*};
use address::WorldAddressExt;
use agent_core::{Agent, WorldAgentExt};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use client_core::{ClientDisconnected, ClientFrame, ConnectionId, WorldClientExt};
use core_plugin::{Entity, World};
use futures_util::{
    FutureExt,
    stream::{FuturesUnordered, StreamExt},
};
use margatroid_protocol::*;
use serde_json::{Value, json};
use std::{
    any::Any,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use storage_core::{AgentStorage, RecordRange};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::Workspace;

struct ReplyOutput {
    value: Value,
    request: Value,
}
type ReplyFuture = Pin<Box<dyn Future<Output = ReplyOutput> + Send>>;
fn trace_response(connection: ConnectionId, request: &Value, response: &Value) {
    if response["success"] != false {
        return;
    }
    let failure = &response["error"];
    let message = failure["details"]["message"]
        .as_str()
        .or_else(|| failure["message"].as_str())
        .unwrap_or("request failed");
    let route = request["type"].as_str().unwrap_or("protocol");
    let id = response["id"].as_str().unwrap_or("");
    let code = failure["code"].as_str().unwrap_or("Unknown");
    if code == "AgentUiNotReady" {
        tracing::warn!(
            connection = connection.get(),
            request_id = id,
            route,
            code,
            "{message}"
        );
    } else {
        tracing::error!(
            connection = connection.get(),
            request_id = id,
            route,
            code,
            "{message}"
        );
    }
}
fn error(code: &str, message: &str) -> ProtocolErrorDto {
    ProtocolErrorDto::new(code, message)
}
trait BoundaryError: Sized + Send + std::fmt::Debug + 'static {
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
domain_error!(
    tool_core::ToolError,
    tool_core::ToolErrorKind::InvalidRequest
);
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
        .filter(|_| {
            world
                .get_resource::<Clients>()
                .and_then(|clients| clients.get(connection))
                .is_some_and(|sender| !sender.is_closed())
        })
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
    original: Value,
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
        ReplyOutput {
            value,
            request: original,
        }
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
        MclValue::Turn(value) => return Ok(json!({"data":{"kind":"turn","value":value}})),
        MclValue::Error(v) => {
            let mut failure = error(
                "MclFailed",
                v["message"].as_str().unwrap_or("MCL execution failed"),
            );
            failure.details = Some(v);
            return Err(failure);
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
pub(crate) fn history_record(v: storage_core::StoredHistoryRecord) -> Value {
    let row = v.record;
    json!({"sequence":v.sequence,"turn":row.turn,"kind":"message","content":row.content,"payload":row.payload,"created_at_ms":row.created_at_ms.unwrap_or(0)})
}
pub(crate) fn mcl_record(v: storage_core::StoredHistoryRecord) -> Value {
    let mut row = json!({"sequence":v.sequence,"turn":v.record.turn,"source":v.record.source.unwrap_or_default(),"content":v.record.content,"payload":v.record.payload,"created_at_ms":v.record.created_at_ms.unwrap_or(0)});
    clean_internal(&mut row["payload"]);
    row
}
pub(crate) fn log_record(v: storage_core::StoredAgentLog) -> Value {
    let mut row = serde_json::to_value(v.record).unwrap();
    row["sequence"] = json!(v.sequence);
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
        "resource.req" => resources(world, object),
        "workspace.create" => dispatch::<CreateWorkspaceDto>(world, object),
        "workspace.delete" => dispatch::<DeleteWorkspaceDto>(world, object),
        "workspace.rename" => rename_alias(world, object, false),
        "agent.rename" => rename_alias(world, object, true),
        "agent.start" => dispatch::<AgentStartDto>(world, object),
        "agent.stop" => dispatch::<AgentStopDto>(world, object),
        "agent.archive" => dispatch::<AgentArchiveDto>(world, object),
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
        "model.config.req" => model_config(world, object, false),
        "model.config.update" => model_config(world, object, true),
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
        world
            .get_resource::<Clients>()
            .and_then(|c| c.get(r.ticket.connection))
            .ok_or_else(|| error("ConnectionMissing", "connection closed"))?;
        Ok(client_core::RegisterClient {
            id: request_id(&r.ticket),
            connection_id: r.ticket.connection,
            client_type: r.input.client_type.clone(),
            name: r.input.name.clone(),
            reply: r.reply.clone(),
        })
    }
);
fn workspace_target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto> {
    let e = target(world, address)?;
    if world.get_component::<Workspace>(e).is_none() {
        return Err(error("WorkspaceMissing", "target is not a Workspace"));
    }
    Ok(e)
}
converter!(
    CreateWorkspaceDto,
    WorkspaceCreateDto,
    Entity,
    workspace_core::WorkspaceError,
    workspace_core::CreateWorkspace,
    |world: &World, r: &Prepared<WorkspaceCreateDto, Entity, workspace_core::WorkspaceError>| {
        client(world, r.ticket.connection)?;
        Ok(workspace_core::CreateWorkspace {
            address: r.input.address.clone(),
            alias: r.input.alias.clone(),
            project_root: r.input.project_root.clone().into(),
            reply: r.reply.clone(),
        })
    }
);
converter!(
    DeleteWorkspaceDto,
    WorkspaceTargetDto,
    (),
    workspace_core::WorkspaceError,
    workspace_core::DeleteWorkspace,
    |world: &World, r: &Prepared<WorkspaceTargetDto, (), workspace_core::WorkspaceError>| {
        client(world, r.ticket.connection)?;
        Ok(workspace_core::DeleteWorkspace {
            workspace: workspace_target(world, &r.input.address)?,
            reply: r.reply.clone(),
        })
    }
);
converter!(AgentStartDto,AgentStartInputDto,tool_core::ToolExecuteResponse,tool_core::ToolError,workspace_core::StartWorkspaceAgent<tool_core::ToolExecuteResponse,tool_core::ToolError>,
|world:&World,r:&Prepared<AgentStartInputDto,tool_core::ToolExecuteResponse,tool_core::ToolError>|{
 client(world,r.ticket.connection)?;
 Ok(match &r.input {
  AgentStartInputDto::Existing{workspace,node_name}=>workspace_core::StartWorkspaceAgent::Existing{workspace:workspace_target(world,workspace)?,node_name:node_name.clone(),reply:r.reply.clone()},
  AgentStartInputDto::New{workspace,tool_path,arguments}=>workspace_core::StartWorkspaceAgent::New{workspace:workspace_target(world,workspace)?,tool_path:tool_path.clone().into(),arguments:arguments.clone(),reply:r.reply.clone()},
 })
});
converter!(
    AgentStopDto,
    AgentTargetDto,
    (),
    agent_core::AgentError,
    agent_core::StopAgent,
    |world: &World, r: &Prepared<AgentTargetDto, (), agent_core::AgentError>| {
        client(world, r.ticket.connection)?;
        agent(world, &r.input.address)?;
        Ok(agent_core::StopAgent {
            address: r.input.address.clone(),
            reply: r.reply.clone(),
        })
    }
);
converter!(
    AgentArchiveDto,
    AgentArchiveInputDto,
    (),
    workspace_core::WorkspaceError,
    workspace_core::ArchiveAgent,
    |world: &World, r: &Prepared<AgentArchiveInputDto, (), workspace_core::WorkspaceError>| {
        client(world, r.ticket.connection)?;
        Ok(workspace_core::ArchiveAgent {
            workspace: workspace_target(world, &r.input.workspace)?,
            node_name: r.input.node_name.clone(),
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
pub(crate) fn rename_alias(world: &World, object: Box<dyn Any + Send>, agent: bool) {
    enum Rename {
        Workspace(String),
        Agent { id: String, alias: Option<String> },
    }
    let (ticket, workspace, rename, reply) = if agent {
        let Ok(request) =
            object.downcast::<Prepared<AgentRenameDto, (), workspace_core::WorkspaceError>>()
        else {
            return;
        };
        (
            request.ticket,
            request.input.workspace,
            Rename::Agent {
                id: request.input.agent_id,
                alias: request.input.alias,
            },
            request.reply,
        )
    } else {
        let Ok(request) =
            object.downcast::<Prepared<WorkspaceRenameDto, (), workspace_core::WorkspaceError>>()
        else {
            return;
        };
        (
            request.ticket,
            request.input.address,
            Rename::Workspace(request.input.alias),
            request.reply,
        )
    };
    let database = (|| {
        client(world, ticket.connection).map_err(workspace_core::WorkspaceError::boundary)?;
        let entity = world.entity_by_address(&workspace).map_err(|_| {
            workspace_core::WorkspaceError::new(
                workspace_core::WorkspaceErrorKind::WorkspaceMissing,
                "Workspace is not loaded",
            )
        })?;
        let workspace = world.get_component::<Workspace>(entity).ok_or_else(|| {
            workspace_core::WorkspaceError::new(
                workspace_core::WorkspaceErrorKind::WorkspaceMissing,
                "target is not a Workspace",
            )
        })?;
        if workspace.state != workspace_core::WorkspaceState::Running {
            return Err(workspace_core::WorkspaceError::new(
                workspace_core::WorkspaceErrorKind::StateRejected,
                "Workspace is not running",
            ));
        }
        Ok(workspace.database.clone())
    })();
    let database = match database {
        Ok(database) => database,
        Err(error) => {
            deliver(&reply, Err(error));
            return;
        }
    };
    let guard = world.get_resource::<DtoState>().unwrap().lifetime.track();
    world
        .get_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
        .unwrap()
        .spawn(async move {
            let _guard = guard;
            let result = tokio::task::spawn_blocking(move || match rename {
                Rename::Workspace(alias) => database.rename_workspace_alias(alias),
                Rename::Agent { id, alias } => database.rename_agent_alias(&id, alias),
            })
            .await
            .unwrap_or_else(|_| {
                Err(workspace_core::WorkspaceError::new(
                    workspace_core::WorkspaceErrorKind::DatabaseFailed,
                    "alias update worker failed",
                ))
            });
            deliver(&reply, result);
        });
}

pub(crate) fn resources(world: &World, object: Box<dyn Any + Send>) {
    let Ok(request) = object.downcast::<Prepared<ResourceQueryDto, Vec<ResourceInfoDto>>>() else {
        return;
    };
    if let Err(failure) = client(world, request.ticket.connection) {
        reject(*request, failure);
        return;
    }
    let Some(roots) = world.get_resource::<address::ResourceRoots>().cloned() else {
        reject(
            *request,
            error(
                "ResourceQueryUnavailable",
                "resource roots are not configured",
            ),
        );
        return;
    };
    let guard = world.get_resource::<DtoState>().unwrap().lifetime.track();
    world
        .get_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
        .unwrap()
        .spawn(async move {
            let _guard = guard;
            let input = request.input;
            let result = tokio::task::spawn_blocking(move || {
                let query = address::ResourceQuery {
                    resource_type: input.resource_type,
                    scope: input.scope,
                    name: input.name,
                };
                let files = address::query_resources(&roots, &query).map_err(|_| {
                    error("ResourceReadFailed", "unable to read resource directories")
                })?;
                files
                    .into_iter()
                    .map(|file| {
                        let path = file
                            .path
                            .to_str()
                            .ok_or_else(|| {
                                error("ResourceReadFailed", "resource path is not UTF-8")
                            })?
                            .to_owned();
                        let tool_path = file
                            .tool_path
                            .map(|path| {
                                path.to_str().map(str::to_owned).ok_or_else(|| {
                                    error("ResourceReadFailed", "resource tool path is not UTF-8")
                                })
                            })
                            .transpose()?;
                        Ok(ResourceInfoDto {
                            address: file.address.to_string(),
                            resource_type: file.address.kind,
                            scope: file.address.scope,
                            name: file.address.name,
                            tag: file.address.tag,
                            path,
                            tool_path,
                        })
                    })
                    .collect::<Result<Vec<_>, ProtocolErrorDto>>()
            })
            .await
            .unwrap_or_else(|_| Err(error("ResourceReadFailed", "resource query worker failed")));
            deliver(&request.reply, result);
        });
}

fn model_config(world: &World, object: Box<dyn Any + Send>, write: bool) {
    macro_rules! operation {
        ($input:ty,$out:ty,$body:expr) => {{
            let Ok(request) = object.downcast::<Prepared<$input, $out>>() else {
                return;
            };
            if let Err(e) = client(world, request.ticket.connection) {
                reject(*request, e);
                return;
            }
            let Some(source) = world
                .get_resource::<crate::probes::ModelConfigView>()
                .cloned()
            else {
                reject(
                    *request,
                    error(
                        "ModelConfigMissing",
                        "model configuration file is unavailable",
                    ),
                );
                return;
            };
            let guard = world.get_resource::<DtoState>().unwrap().lifetime.track();
            world.send_async_closure(
                app_runtime_plugin::RuntimePlugin::UPDATE,
                move || async move {
                    let _guard = guard;
                    let result = ($body)(source, &request.input).await;
                    deliver(&request.reply, result);
                    Ok::<_, DtoAsyncError>(())
                },
            );
        }};
    }
    if write {
        operation!(
            ModelConfigUpdateDto,
            (),
            |source: crate::probes::ModelConfigView, input: &ModelConfigUpdateDto| {
                let content = input.content.clone();
                async move { source.replace(content).await }
            }
        );
    } else {
        operation!((), String, |source: crate::probes::ModelConfigView,
                                _: &()| async move {
            source.read().await
        });
    }
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
            .filter(|handle| handle.turn.as_deref() == Some(r.input.turn.as_str()))
        {
            active.cancel();
        }
        if let Some(tasks) = world.get_component::<tool_core::ToolTasks>(entity) {
            tasks.abort_unreturned(&r.input.turn);
        }
        Ok(())
    })();
    deliver(&r.reply, result);
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
    let original = serde_json::to_value(&message).unwrap();
    let route = original["type"].as_str().unwrap().to_owned();
    let id = message.id().to_owned();
    let response = if route.ends_with(".req") {
        format!("{}res", &route[..route.len() - 3])
    } else {
        format!("{route}.result")
    };
    let ticket = state.clients.claim(connection, &id).ok_or_else(|| {
        frame(
            &response,
            &id,
            Err(error("DuplicateRequest", "request id is already in flight")),
        )
    })?;
    macro_rules! build {
        ($input:expr,$out:ty,$err:ty,$project:expr) => {{
            let (event, waiter) = request::<_, $out, $err>(
                ticket,
                &route,
                $input,
                &response,
                $project,
                original.clone(),
            );
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
        ClientMessage::ModelConfigReq { .. } => build!((), String, ProtocolErrorDto, |content| Ok(
            json!({"content":content})
        )),
        ClientMessage::ModelConfigUpdate { message, .. } => {
            build!(message, (), ProtocolErrorDto, minimal)
        }
        ClientMessage::ResourceReq { message, .. } => build!(
            message,
            Vec<ResourceInfoDto>,
            ProtocolErrorDto,
            |resources| Ok(json!({"resources":resources}))
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
        ClientMessage::WorkspaceCreate { message, .. } => {
            build!(message, Entity, workspace_core::WorkspaceError, minimal)
        }
        ClientMessage::WorkspaceDelete { message, .. } => {
            build!(message, (), workspace_core::WorkspaceError, minimal)
        }
        ClientMessage::WorkspaceRename { message, .. } => {
            build!(message, (), workspace_core::WorkspaceError, minimal)
        }
        ClientMessage::AgentRename { message, .. } => {
            build!(message, (), workspace_core::WorkspaceError, minimal)
        }
        ClientMessage::AgentStart { message, .. } => build!(
            message,
            tool_core::ToolExecuteResponse,
            tool_core::ToolError,
            |response: tool_core::ToolExecuteResponse| {
                if response.failed {
                    return Err(error("ExecutionFailed", &response.content));
                }
                minimal(())
            }
        ),
        ClientMessage::AgentStop { message, .. } => {
            build!(message, (), agent_core::AgentError, minimal)
        }
        ClientMessage::AgentArchive { message, .. } => {
            build!(message, (), workspace_core::WorkspaceError, minimal)
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
                let value = frame(
                    "agent.turn.abort.result",
                    &ticket.id,
                    result.map(|()| json!({})),
                );
                ReplyOutput {
                    value,
                    request: original,
                }
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
pub(crate) async fn workspace_file_handler(
    axum::extract::State(state): axum::extract::State<DtoState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    method: axum::http::Method,
    path: Result<axum::extract::Path<(String, String)>, axum::extract::rejection::PathRejection>,
) -> axum::response::Response {
    use axum::{http::StatusCode as Status, response::IntoResponse};
    let run = async {
        let header = |name| {
            headers
                .get(name)
                .map(|value| {
                    value
                        .to_str()
                        .map(str::to_owned)
                        .map_err(|_| Status::FORBIDDEN)
                })
                .transpose()
        };
        state
            .ingress
            .evaluate(&config_core::IngressRequest {
                peer,
                origin: header("origin")?,
                host: header("host")?,
            })
            .map_err(|_| Status::FORBIDDEN)?;
        if state.events.get().is_none() || state.lifetime.is_cancelled() {
            return Err(Status::SERVICE_UNAVAILABLE);
        }
        let raw = uri.path().as_bytes();
        let mut offset = 0;
        while offset < raw.len() {
            if raw[offset] == b'%' {
                if raw
                    .get(offset + 1)
                    .is_none_or(|byte| !byte.is_ascii_hexdigit())
                    || raw
                        .get(offset + 2)
                        .is_none_or(|byte| !byte.is_ascii_hexdigit())
                {
                    return Err(Status::BAD_REQUEST);
                }
                offset += 3;
            } else {
                offset += 1;
            }
        }
        let axum::extract::Path((workspace, path)) = path.map_err(|_| Status::BAD_REQUEST)?;
        let address = address::Address::new(&workspace).map_err(|_| Status::BAD_REQUEST)?;
        if address.as_str() != workspace {
            return Err(Status::BAD_REQUEST);
        }
        validate_workspace_file_path(&path)?;
        let (tx, rx) = oneshot::channel();
        state.world.send(move |world| {
            let result = world
                .entity_by_address(&workspace)
                .ok()
                .and_then(|entity| world.get_component::<Workspace>(entity))
                .ok_or(Status::NOT_FOUND)
                .and_then(|workspace| {
                    if workspace.state == workspace_core::WorkspaceState::Deleting {
                        Err(Status::CONFLICT)
                    } else {
                        Ok(workspace.project_root.clone())
                    }
                });
            let _ = tx.send(result);
        });
        let root = tokio::select! {
            biased;
            _=state.lifetime.cancelled()=>return Err(Status::SERVICE_UNAVAILABLE),
            root=rx=>root.map_err(|_|Status::SERVICE_UNAVAILABLE)??,
        };
        if state.lifetime.is_cancelled() {
            return Err(Status::SERVICE_UNAVAILABLE);
        }
        let head_only = method == axum::http::Method::HEAD;
        // The worker owns the service guard even if the HTTP requester disconnects.
        let guard = state.lifetime.track();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            read_workspace_file(root, path, head_only)
        })
        .await
        .map_err(|_| Status::INTERNAL_SERVER_ERROR)?
    };
    match run.await {
        Ok(response) => response,
        Err(status) => status.into_response(),
    }
}

fn validate_workspace_file_path(path: &str) -> Result<(), axum::http::StatusCode> {
    use axum::http::StatusCode as Status;
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path.as_bytes().get(1) == Some(&b':')
    {
        return Err(Status::BAD_REQUEST);
    }
    for part in path.split('/') {
        if part == ".." {
            return Err(Status::FORBIDDEN);
        }
        if part.is_empty() || part == "." {
            return Err(Status::BAD_REQUEST);
        }
    }
    Ok(())
}

fn workspace_file_status(error: std::io::Error) -> axum::http::StatusCode {
    use axum::http::StatusCode as Status;
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => Status::NOT_FOUND,
        std::io::ErrorKind::PermissionDenied => Status::FORBIDDEN,
        _ => {
            #[cfg(unix)]
            if error.raw_os_error() == Some(libc::ELOOP) {
                return Status::FORBIDDEN;
            }
            Status::INTERNAL_SERVER_ERROR
        }
    }
}

#[cfg(unix)]
fn open_workspace_file(
    root: &std::path::Path,
    path: &str,
) -> Result<std::fs::File, axum::http::StatusCode> {
    use axum::http::StatusCode as Status;
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::OpenOptionsExt,
    };
    let mut parent = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(root)
        .map_err(workspace_file_status)?;
    let parts = path.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        let name = std::ffi::CString::new(*part).map_err(|_| Status::BAD_REQUEST)?;
        let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
        // Inspect relative to the already open parent; never follow a symbolic link.
        let result = unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                info.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(workspace_file_status(std::io::Error::last_os_error()));
        }
        // fstatat initialized the entire stat value on success.
        let mode = unsafe { info.assume_init() }.st_mode & libc::S_IFMT;
        if mode == libc::S_IFLNK {
            return Err(Status::FORBIDDEN);
        }
        let last = index + 1 == parts.len();
        if (last && mode != libc::S_IFREG) || (!last && mode != libc::S_IFDIR) {
            return Err(Status::NOT_FOUND);
        }
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if last { 0 } else { libc::O_DIRECTORY };
        // Each new descriptor is anchored to its parent and no-follow closes the stat/open race.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(workspace_file_status(std::io::Error::last_os_error()));
        }
        // A successful openat transfers this fresh descriptor to File exactly once.
        parent = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    if !parent.metadata().map_err(workspace_file_status)?.is_file() {
        return Err(Status::NOT_FOUND);
    }
    Ok(parent)
}

fn read_workspace_file(
    root: PathBuf,
    path: String,
    head_only: bool,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    use axum::{
        body::Body,
        http::{StatusCode as Status, header},
    };
    validate_workspace_file_path(&path)?;
    #[cfg(not(unix))]
    {
        let _ = (root, path, head_only);
        Err(Status::NOT_IMPLEMENTED)
    }
    #[cfg(unix)]
    {
        let file = open_workspace_file(&root, &path)?;
        let length = file.metadata().map_err(workspace_file_status)?.len();
        let media_type = match std::path::Path::new(&path)
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            Some("avif") => "image/avif",
            Some("svg") => "image/svg+xml",
            Some("ico") => "image/x-icon",
            Some("bmp") => "image/bmp",
            Some("html" | "htm") => "text/html; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("js" | "mjs") => "text/javascript; charset=utf-8",
            Some("json") => "application/json",
            Some("txt" | "md" | "log" | "csv" | "lua" | "toml" | "yaml" | "yml" | "rs") => {
                "text/plain; charset=utf-8"
            }
            Some("pdf") => "application/pdf",
            Some("woff") => "font/woff",
            Some("woff2") => "font/woff2",
            Some("mp4") => "video/mp4",
            Some("mp3") => "audio/mpeg",
            _ => "application/octet-stream",
        };
        let body = if head_only {
            Body::empty()
        } else {
            let reader = tokio::io::AsyncReadExt::take(tokio::fs::File::from_std(file), length);
            Body::from_stream(tokio_util::io::ReaderStream::new(reader))
        };
        axum::response::Response::builder()
            .status(Status::OK)
            .header(header::CONTENT_TYPE, media_type)
            .header(header::CONTENT_LENGTH, length)
            .header(header::CACHE_CONTROL, "no-store")
            .header("x-content-type-options", "nosniff")
            .body(body)
            .map_err(|_| Status::INTERNAL_SERVER_ERROR)
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
    let Some((connection, outbound, cancel)) = state.clients.allocate() else {
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
        self.state.clients.remove(self.connection);
        if let Some(events) = self.state.events.get() {
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
        "log.req"
            | "log.append"
            | "connection.register"
            | "model.config.req"
            | "model.config.update"
            | "workspace.req"
            | "workspace.start"
            | "workspace.stop"
            | "workspace.restart"
            | "workspace.create"
            | "workspace.update"
            | "workspace.rename"
            | "workspace.delete"
            | "agent.req"
            | "agent.rename"
            | "agent.ui.req"
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
    mut pushes: client_core::ClientReceiver,
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
                let Some(report)=response else{continue;};
                trace_response(connection,&report.request,&report.value);
                match encode_response(report.value).ok_or(()) {
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
                            let original=serde_json::to_value(&message).unwrap();
                            let state=state.clone();
                            pending.push(async move {
                                match prepare_request(&state,connection,message).await {
                                    Ok((request,abort,future))=>{
                                        drop(original);
                                        let events=state.events.get().unwrap();
                                        if let Some(v)=request{events.send_event(v);}
                                        if let Some(v)=abort{events.send_event(v);}
                                        future.await
                                    },
                                    Err(value)=>ReplyOutput{value,request:original},
                                }
                            }.boxed());
                            None
                        },
                        Err(error)=>{
                            let request=serde_json::from_str(&text).unwrap_or_else(|_|json!({"raw_frame":text.to_string()}));
                            if let Some(value)=invalid_reply(&text){pending.push(async move{ReplyOutput{value,request}}.boxed());None}
                            else{let failure=json!({"success":false,"error":{"code":error.0.code,"message":error.0.message}});trace_response(connection,&request,&failure);Some(Message::Close(Some(CloseFrame{code:if text.len()>MAX_MESSAGE_BYTES{1009}else{1007},reason:"invalid protocol request".into()})))}
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
    state.clients.remove(connection);
    pushes.close().await;
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
    #[test]
    fn mcl_error_summary_keeps_the_complete_original_report_in_details() {
        let message = format!("{} END-OF-COMPLETE-REPORT", "完整错误".repeat(300));
        let original = serde_json::json!({"type":"error","message":message,"request_id":"mcl-original-request"});
        let error = mcl_result(mcl_core::MclValue::Error(original.clone())).unwrap_err();
        assert!(error.message.len() <= 512);
        assert_eq!(error.details, Some(original));
        assert!(format!("{error:?}").contains("END-OF-COMPLETE-REPORT"));
    }
}
