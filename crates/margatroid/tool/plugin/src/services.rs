//! Host services shared by automatic Lua tasks and tool worker requests.
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::{App, World};
use futures_util::FutureExt;
use lua_runtime_core::{LuaExecutionIdentity, LuaRuntimeError, LuaRuntimeErrorKind, LuaServices};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;
#[cfg(test)]
#[path = "../tests/internal/creation.rs"]
mod creation_tests;

fn failure(message: impl ToString) -> LuaRuntimeError {
    LuaRuntimeError::new(LuaRuntimeErrorKind::ExecutionFailed, message.to_string())
}
async fn world<T: Send + 'static>(
    sender: Arc<WorldClosureSender>,
    operation: impl FnOnce(&mut World) -> Result<T, LuaRuntimeError> + Send + 'static,
) -> Result<T, LuaRuntimeError> {
    let (tx, rx) = oneshot::channel();
    sender.send(move |world| {
        let _ = tx.send(operation(world));
    });
    rx.await.map_err(failure)?
}
pub fn install(app: &mut App) {
    install_on(app, RuntimePlugin::UPDATE)
}
pub(crate) fn install_on(app: &mut App, schedule: &str) {
    if app.world().contains_resource::<LuaServices>() {
        return;
    }
    let sender = Arc::new(app.world().closure_sender(schedule));
    let weak = Arc::downgrade(&sender);
    app.world_mut()
        .insert_resource(ExecutionBindings::default());
    app.world_mut()
        .insert_resource(ServiceOwner { _sender: sender });
    app.world_mut()
        .insert_resource(LuaServices::new(move |identity, method, arguments| {
            let sender = weak.upgrade();
            Box::pin(async move {
                let sender = sender.ok_or_else(|| failure("Lua host stopped"))?;
                invoke(sender, identity, method, arguments).await
            })
        }));
}
struct ServiceOwner {
    _sender: Arc<WorldClosureSender>,
}
impl core_plugin::Resource for ServiceOwner {}
async fn invoke(
    sender: Arc<WorldClosureSender>,
    identity: LuaExecutionIdentity,
    method: String,
    arguments: Value,
) -> Result<Value, LuaRuntimeError> {
    if method == "execution_finished" {
        return finish_execution(sender, identity, arguments).await;
    }
    if identity.cancel.is_cancelled() {
        return Err(failure("Lua execution cancelled"));
    }
    match method.as_str(){
  "workspace_agents"=>world(sender,move|world|{validate_execution(world,&identity)?;Ok(json!(world.query_with::<agent_core::Agent>().result().into_iter().filter_map(|entity|{let agent=world.get_component::<agent_core::Agent>(entity)?;if agent.workspace()!=identity.workspace{return None;}Some(json!({"address":world.get_component::<address::Address>(entity)?.as_str(),"state":format!("{:?}",agent.state()).to_lowercase()}))}).collect::<Vec<_>>()))}).await,
  "tools_register"=>{
   let registration:tool_resource_core::RegisterToolsRequest=serde_json::from_value(arguments).map_err(failure)?;
   let checked=identity.clone();let source=identity.source;let instance=identity.instance.clone();let weak=Arc::downgrade(&sender);
   let (task,path)=world(sender,move|world|{validate_execution(world,&checked)?;let task=world.get_component::<tool_core::LuaExecutionTask>(source).ok_or_else(||failure("execution task missing"))?;if world.get_component::<lua_runtime_core::DriverLuaVm>(source).is_none_or(|vm|vm.instance.start_id!=instance){return Err(failure("execution identity changed"));}Ok((task.handle.clone(),task.resource_dir.clone()))}).await?;
   super::handler::register_from_execution(weak,source,identity.instance,task,path,registration).await.map_err(failure)?;Ok(Value::Null)
  }
  "task_return_result"|"task_append_output"|"task_replace_output"=>{
   world(sender,move|world|{validate_execution(world,&identity)?;let task=world.get_component::<tool_core::LuaExecutionTask>(identity.source).ok_or_else(||failure("execution task missing"))?;
    match method.as_str(){"task_return_result"=>task.handle.return_result(tool_core::parse_tool_result(&arguments)).map_err(failure)?,"task_append_output"=>task.handle.write(arguments).map_err(failure)?,_=>task.handle.replace(arguments).map_err(failure)?,}
     if method=="task_return_result"{acknowledge_execution(world,&identity);}Ok(Value::Null)
   }).await
  }
  "task_receive"=>{
   let cancel=identity.cancel.clone();let input=world(sender,move|world|{validate_execution(world,&identity)?;Ok(world.get_component::<tool_core::LuaExecutionTask>(identity.source).ok_or_else(||failure("execution task missing"))?.input.clone())}).await?;
   tokio::select!{biased;_=cancel.cancelled()=>Err(failure("execution cancelled")),value=async{input.lock().await.recv().await}=>value.ok_or_else(||failure("execution mailbox closed"))}
  }
  "task_read_output"|"task_send"|"task_finish"=>{
   let caller_cancel=identity.cancel.clone();
    let value=if method=="task_send"{arguments.get("target").cloned().ok_or_else(||failure("task target missing"))?}else{arguments.clone()};
   let target=if let Some(id)=value.as_str(){tool_resource_core::TaskReference{agent:None,task_id:id.into()}}else{serde_json::from_value::<tool_resource_core::TaskReference>(value).map_err(failure)?};
   let source=identity.source;let method_copy=method.clone();let target_handle=world(sender,move|world|{validate_execution(world,&identity)?;
    let entity=if let Some(address)=target.agent{let address=address::Address::new(&address).map_err(failure)?;world.query_with::<address::Address>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&address)).ok_or_else(||failure("task Agent missing"))?}else{source};
    if super::handler::workspace_of(world,entity).map_err(failure)?!=identity.workspace{return Err(failure("task outside Workspace"));}
    let handle=world.get_component::<tool_core::ToolTasks>(entity).ok_or_else(||failure("tasks missing"))?.get(&target.task_id).map_err(failure)?;
    if method_copy=="task_finish"&&entity==source&&handle.execution_id()==identity.instance{handle.cancel();identity.cancel.cancel();return Err(failure("execution finished"));}Ok(handle)
   }).await?;
   match method.as_str(){"task_read_output"=>serde_json::to_value(target_handle.read().map_err(failure)?).map_err(failure),"task_send"=>{target_handle.send(arguments.get("value").cloned().unwrap_or(Value::Null)).map_err(failure)?;Ok(Value::Null)},_=>{let finished=target_handle.finish();tokio::select!{biased;_=caller_cancel.cancelled()=>Err(failure("caller cancelled while finishing target task")),result=finished=>{result.map_err(failure)?;Ok(Value::Null)}}}}
  }
  "execution_log"=>{
    let mut record:storage_core::AgentLogRecord=serde_json::from_value(arguments).map_err(failure)?;
    record.source=if identity.instance.starts_with("task-instance:"){storage_core::AgentLogSource::Tool}else{storage_core::AgentLogSource::Driver};
    let mut trusted=record.context_json.take().unwrap_or_else(||json!({}));
    let values=trusted.as_object_mut().ok_or_else(||failure("log context must be object"))?;
    values.insert("instance".into(),json!(identity.instance));values.insert("source_address".into(),json!(identity.address));record.context_json=Some(trusted);
    let agents=world(sender,move|world|{validate_execution(world,&identity)?;let agents=world.get_resource::<ExecutionBindings>().unwrap().diagnostics.lock().unwrap().get(&identity.instance).map(|agents|agents.values().cloned().collect::<Vec<_>>()).unwrap_or_default();Ok(agents.into_iter().map(|agent|{let context=bound_context(world,&agent);(agent,context)}).collect::<Vec<_>>())}).await?;
    let count=agents.len();for (agent,context) in agents{let mut record=record.clone();let mut full=record.context_json.take().unwrap_or_else(||json!({}));full.as_object_mut().ok_or_else(||failure("log context must be object"))?.extend(context.as_object().unwrap().clone());record.context_json=Some(full);tokio::task::spawn_blocking(move||storage_core::append_agent_log(&agent.storage,record)).await.map_err(failure)?.map_err(failure)?;}Ok(json!(count))
   }
  "agent_data_dir"=>{
     let target=arguments.as_str().filter(|value|!value.trim().is_empty()).ok_or_else(||failure("agent.data_dir requires an Agent address"))?.to_owned();
     world(sender,move|world|{
       validate_execution(world,&identity)?;
       let address=address::Address::new(&target).map_err(failure)?;
       let entity=world.query_with::<agent_core::Agent>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&address)).ok_or_else(||failure("Agent data directory target missing"))?;
       if world.get_component::<agent_core::Agent>(entity).unwrap().workspace()!=identity.workspace{return Err(failure("Agent data directory target outside Workspace"));}
       let storage=world.get_component::<storage_core::AgentStorage>(entity).ok_or_else(||failure("Agent storage missing"))?;
       Ok(json!(storage.data_dir()))
     }).await
   }
   "agent_stop"=>{
    let target=arguments.as_str().ok_or_else(||failure("agent.stop requires Agent address"))?.to_owned();let(tx,rx)=oneshot::channel();
    world(sender,move|world|{validate_execution(world,&identity)?;let parsed=address::Address::new(&target).map_err(failure)?;
     let entity=world.query_with::<agent_core::Agent>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&parsed)).ok_or_else(||failure("stop Agent missing"))?;
     if world.get_component::<agent_core::Agent>(entity).unwrap().workspace()!=identity.workspace{return Err(failure("stop target outside Workspace"));}
     world.send_event(agent_core::StopAgent{address:target,reply:Arc::new(std::sync::Mutex::new(Some(tx)))});
     // Stop may cancel the caller's own tool task: supervise the original reply externally.
     async_runtime_plugin::WorldAsyncExt::send_async_closure(world,RuntimePlugin::UPDATE,move||async move{match rx.await{Ok(Ok(_))=>{},other=>tracing::error!(result=?other,"injected Agent stop failed")};Ok::<(),async_runtime_plugin::AsyncTaskError>(())});Ok(())
    }).await?;Ok(json!({}))
   }
     "agent_create"=> create_agent(sender, identity, arguments).await,
"wait"=>{
   let target=arguments.as_str().ok_or_else(||failure("wait requires Agent address"))?.to_owned();let cancel=identity.cancel.clone();
   let slot=world(sender,move|world|{validate_execution(world,&identity)?;let address=address::Address::new(&target).map_err(failure)?;let entity=world.query_with::<address::Address>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&address)).ok_or_else(||failure("wait Agent missing"))?;
    if world.get_component::<agent_core::Agent>(entity).is_none_or(|agent|agent.workspace()!=identity.workspace){return Err(failure("wait target outside Workspace"));}
    acknowledge_execution(world,&identity);
     Ok(world.get_component::<agent_core::Agent>(entity).ok_or_else(||failure("Agent missing"))?.wake_slot())
   }).await?;
   tokio::select!{biased;_=cancel.cancelled()=>Err(failure("wait cancelled")),result=agent_core::AgentWakeSlot::wait(slot)=>{result.map_err(failure)?;Ok(Value::Null)}}
  }
  "mcl_execute"=>{
   let target=arguments.get("target").and_then(Value::as_str).ok_or_else(||failure("MCL target address required"))?.to_owned();
   let text=arguments.get("instruction").and_then(Value::as_str).ok_or_else(||failure("MCL instruction required"))?.to_owned();
   let parameters=arguments.get("arguments").cloned().unwrap_or_else(||json!([]));let parameters=serde_json::from_value::<Vec<Value>>(parameters).map_err(failure)?;
   let cancel=identity.cancel.clone();let(tx,rx)=oneshot::channel();
   world(sender,move|world|{validate_execution(world,&identity)?;
    let target=address::Address::new(&target).map_err(failure)?;
    let entity=world.query_with::<address::Address>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&target)).ok_or_else(||failure("MCL target missing"))?;
    let agent=world.get_component::<agent_core::Agent>(entity).ok_or_else(||failure("MCL target is not Agent"))?;
    if agent.workspace()!=identity.workspace{return Err(failure("MCL target belongs to another Workspace"));}
    world.send_event(mcl_core::MclCommand{id:format!("lua:{}:{}",identity.instance,next()),source:identity.source,source_address:identity.address,source_instance:Some(identity.instance),target:entity,instruction:mcl_core::MclInstruction{text,arguments:parameters},cancel:identity.cancel,reply:Arc::new(std::sync::Mutex::new(Some(tx)))});Ok(())
   }).await?;
   tokio::select!{biased;_=cancel.cancelled()=>Err(failure("MCL cancelled")),result=rx=>{let value=result.map_err(failure)?.map_err(failure)?;Ok(mcl_value(value))}}
  }
  _=>Err(failure(format!("unknown Lua host operation {method}"))),
 }
}
fn next() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
fn mcl_value(value: mcl_core::MclValue) -> Value {
    match value {
        mcl_core::MclValue::Unit => json!({}),
        mcl_core::MclValue::Turn(turn) => json!({"Turn":turn}),
        mcl_core::MclValue::Config(value) => json!({"Config":value}),
        mcl_core::MclValue::Array(value) => json!({"Array":value}),
        mcl_core::MclValue::Inference {
            message,
            stop_reason,
        } => json!({"Inference":{"message":message,"stop_reason":stop_reason}}),
        mcl_core::MclValue::Import { kind, alias } => json!({"Import":{"kind":kind,"alias":alias}}),
        mcl_core::MclValue::BlockSnapshots(value) => json!({"BlockSnapshots":value}),
        mcl_core::MclValue::Error(value) => json!({"Error":value}),
    }
}

fn acknowledge_execution(world: &World, identity: &LuaExecutionIdentity) {
    if let Some(vm) = world.get_component::<lua_runtime_core::DriverLuaVm>(identity.source) {
        if vm.instance.start_id == identity.instance {
            if let Some(sender) = vm.ready.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = sender.send(Ok(()));
            }
        }
    }
}

fn validate_execution(
    world: &World,
    identity: &LuaExecutionIdentity,
) -> Result<(), LuaRuntimeError> {
    if identity.cancel.is_cancelled()
        || !world.is_alive(identity.source)
        || world
            .get_component::<address::Address>(identity.source)
            .is_none_or(|address| address.as_str() != identity.address)
    {
        return Err(failure("Lua execution is closed or replaced"));
    }
    let current = if identity.instance.starts_with("task-instance:") {
        world
            .get_component::<tool_core::ToolTasks>(identity.source)
            .is_some_and(|tasks| {
                tasks.list().iter().any(|(_, handle)| {
                    handle.execution_id() == identity.instance
                        && !handle.cancel_token().is_cancelled()
                        && !handle.is_cleanup_complete()
                })
            })
    } else {
        world
            .get_component::<lua_runtime_core::DriverLuaVm>(identity.source)
            .is_some_and(|vm| {
                vm.instance.start_id == identity.instance && !vm.cancel.is_cancelled()
            })
    };
    if !current {
        return Err(failure("Lua execution identity is stale"));
    }
    Ok(())
}

#[derive(Clone)]
struct MonitoredAgent {
    entity: core_plugin::Entity,
    session: String,
    storage: storage_core::AgentStorage,
}
#[derive(Clone)]
struct Invocation {
    task_id: String,
    path: Arc<Mutex<PathBuf>>,
    arguments: Value,
}
#[derive(Default)]
struct ExecutionBindings {
    invocations: Mutex<HashMap<String, Invocation>>,
    diagnostics: Arc<Mutex<HashMap<String, BTreeMap<u64, MonitoredAgent>>>>,
    creating: Mutex<HashMap<String, Vec<tokio::sync::watch::Receiver<bool>>>>,
}
impl core_plugin::Resource for ExecutionBindings {}
pub(crate) fn register_invocation(
    world: &World,
    handle: &tool_core::ToolTaskHandle,
    task_id: &str,
    _source: core_plugin::Entity,
    path: &std::path::Path,
    arguments: &Value,
) -> Arc<Mutex<PathBuf>> {
    let path = Arc::new(Mutex::new(path.to_path_buf()));
    world
        .get_resource::<ExecutionBindings>()
        .expect("Lua services installed")
        .invocations
        .lock()
        .unwrap()
        .insert(
            handle.execution_id().into(),
            Invocation {
                task_id: task_id.into(),
                path: path.clone(),
                arguments: arguments.clone(),
            },
        );
    path
}
fn tool_failure(message: impl ToString) -> tool_core::ToolError {
    tool_core::ToolError::new(tool_core::ToolErrorKind::CleanupFailed, message.to_string())
}
fn creator_task(
    world: &World,
    source: core_plugin::Entity,
    instance: &str,
) -> Result<(String, tool_core::ToolTaskHandle), LuaRuntimeError> {
    world
        .get_component::<tool_core::ToolTasks>(source)
        .and_then(|tasks| {
            tasks
                .list()
                .into_iter()
                .find(|(_, h)| h.execution_id() == instance)
        })
        .ok_or_else(|| failure("Agent creator task missing"))
}
fn bound_context(world: &World, agent: &MonitoredAgent) -> Value {
    let current = world
        .get_component::<agent_core::Agent>(agent.entity)
        .filter(|a| a.instance() == agent.session);
    json!({"agent":world.get_component::<address::Address>(agent.entity).map(|a|a.as_str()),"turn":current.and_then(|a|a.turn().clone())})
}
struct CreationDone(tokio::sync::watch::Sender<bool>);
impl Drop for CreationDone {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArguments {
    node_name: Option<String>,
    alias: Option<String>,
    project_root: PathBuf,
    config_path: PathBuf,
    ui_path: PathBuf,
}
struct PreparedCreation {
    database: workspace_core::WorkspaceDatabase,
    operation: Arc<workspace_core::WorkspaceOperation>,
    record: workspace_core::WorkspaceAgent,
    creator: tool_core::ToolTaskHandle,
    task_id: String,
    parent: core_plugin::Entity,
    root: PathBuf,
}
#[derive(Clone)]
struct PendingAgentHandoff {
    world: std::sync::Weak<WorldClosureSender>,
    address: String,
    parent: core_plugin::Entity,
    task_id: String,
    storage: storage_core::AgentStorage,
}
struct Compensation {
    pending_handoff: Option<PendingAgentHandoff>,
    agent: Option<agent_core::AgentHandle>,
    storage: Option<storage_core::AgentStorage>,
    directory: PathBuf,
    remove_directory: bool,
    operation: Option<Arc<workspace_core::WorkspaceOperation>>,
}
async fn compensate(state: Arc<Mutex<Compensation>>) -> Result<(), tool_core::ToolError> {
    let pending = state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pending_handoff
        .clone();
    if let Some(pending) = pending {
        let sender = pending
            .world
            .upgrade()
            .ok_or_else(|| tool_failure("World unavailable while checking failed Agent handoff"))?;
        let close = world(sender, move |world| {
            let matches = world
                .query_with::<storage_core::AgentStorage>()
                .result()
                .into_iter()
                .filter(|entity| {
                    world
                        .get_component::<storage_core::AgentStorage>(*entity)
                        .is_some_and(|s| s.same_database(&pending.storage))
                })
                .collect::<Vec<_>>();
            if matches.is_empty() {
                return Ok(None);
            }
            if matches.len() != 1 {
                return Err(failure(
                    "ambiguous live storage ownership after failed handoff",
                ));
            }
            let entity = matches[0];
            let agent = world
                .get_component::<agent_core::Agent>(entity)
                .ok_or_else(|| failure("live failed-handoff storage has no Agent owner"))?;
            if agent.parent() != pending.parent
                || agent.task_id() != pending.task_id
                || world
                    .get_component::<address::Address>(entity)
                    .is_none_or(|a| a.as_str() != pending.address)
            {
                return Err(failure("live failed-handoff storage ownership mismatch"));
            }
            let task = world
                .get_component::<tool_core::ToolTasks>(pending.parent)
                .ok_or_else(|| failure("failed-handoff parent task collection missing"))?
                .get(&pending.task_id)
                .map_err(failure)?;
            task.child_close(entity)
                .map(Some)
                .ok_or_else(|| failure("live failed-handoff Agent has no retained close callback"))
        })
        .await
        .map_err(tool_failure)?;
        if let Some(close) = close {
            close().await?;
        }
        state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending_handoff = None;
    }
    let agent = state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .agent
        .clone();
    if let Some(agent) = agent {
        agent.close().await.map_err(tool_failure)?;
        state.lock().unwrap_or_else(|e| e.into_inner()).agent = None;
    }
    tokio::task::spawn_blocking(move || {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(storage) = &state.storage {
            storage_core::close_storage(storage).map_err(tool_failure)?;
        }
        state.storage = None;
        if state.remove_directory {
            // Directory is exactly the candidate returned by WorkspaceDatabase::agent_directory.
            let target = state.directory.clone();
            if !target.is_absolute() {
                return Err(tool_failure("cleanup directory is not absolute"));
            }
            match std::fs::remove_dir_all(&target) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(tool_failure(e)),
            }
            state.remove_directory = false;
        }
        state.operation = None;
        Ok(())
    })
    .await
    .map_err(tool_failure)?
}
async fn create_agent(
    sender: Arc<WorldClosureSender>,
    identity: LuaExecutionIdentity,
    arguments: Value,
) -> Result<Value, LuaRuntimeError> {
    let args: CreateArguments = serde_json::from_value(arguments).map_err(failure)?;
    let checked = identity.clone();
    let execution = identity.instance.clone();
    let (done, complete) = tokio::sync::watch::channel(false);
    let invocation = world(sender.clone(), move |world| {
        validate_execution(world, &checked)?;
        let invocation = world
            .get_resource::<ExecutionBindings>()
            .unwrap()
            .invocations
            .lock()
            .unwrap()
            .get(&checked.instance)
            .cloned()
            .ok_or_else(|| failure("trusted Driver invocation missing"))?;
        world
            .get_resource::<ExecutionBindings>()
            .unwrap()
            .creating
            .lock()
            .unwrap()
            .entry(checked.instance)
            .or_default()
            .push(complete);
        Ok(invocation)
    })
    .await?;
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _done = CreationDone(done);
        let result =
            std::panic::AssertUnwindSafe(create_supervised(sender, identity, args, invocation))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err(failure("Agent creation supervisor panicked")));
        let _ = tx.send(result);
    });
    let _ = execution;
    rx.await.map_err(failure)?
}
async fn create_supervised(
    sender: Arc<WorldClosureSender>,
    identity: LuaExecutionIdentity,
    args: CreateArguments,
    invocation: Invocation,
) -> Result<Value, LuaRuntimeError> {
    if let Some(declared) = invocation
        .arguments
        .get("node_name")
        .filter(|v| !v.is_null())
    {
        if declared.as_str() != args.node_name.as_deref() {
            return Err(failure("Driver node_name differs from requested node"));
        }
    }
    let checked = identity.clone();
    let node = args.node_name.clone();
    let alias = args.alias.clone();
    let path = invocation
        .path
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let original = invocation.arguments;
    let prepared = world(sender.clone(), move |world| {
        validate_execution(world, &checked)?;
        let (task_id, creator) = creator_task(world, checked.source, &checked.instance)?;
        let workspace = world
            .get_component::<workspace_core::Workspace>(checked.workspace)
            .ok_or_else(|| failure("Workspace missing"))?;
        if workspace.state != workspace_core::WorkspaceState::Running {
            return Err(failure("Workspace is not running"));
        }
        let database = workspace.database.clone();
        let root = world
            .get_component::<agent_core::Agent>(checked.source)
            .map(|a| a.project_root().to_owned())
            .unwrap_or_else(|| workspace.project_root.clone());
        let key = match &node {
            Some(node) => node.clone(),
            None => database.temporary_operation_key().map_err(failure)?,
        };
        let operation = Arc::new(database.try_operation(&key).map_err(failure)?);
        let record = database
            .prepare_agent(
                &operation,
                checked.address.clone(),
                node,
                alias,
                path,
                original,
            )
            .map_err(failure)?;
        let address = format!(
            "{}/{}",
            checked.address.trim_end_matches('/'),
            record.runtime_name
        );
        if world
            .query_with::<address::Address>()
            .result()
            .into_iter()
            .any(|e| {
                world
                    .get_component::<address::Address>(e)
                    .is_some_and(|a| a.as_str() == address)
            })
        {
            return Err(failure("Agent node already running or closing"));
        }
        Ok(PreparedCreation {
            database,
            operation,
            record,
            creator,
            task_id,
            parent: checked.source,
            root,
        })
    })
    .await?;
    let directory = prepared
        .database
        .agent_directory(&prepared.record)
        .map_err(failure)?;
    let cleanup = Arc::new(Mutex::new(Compensation {
        pending_handoff: None,
        agent: None,
        storage: None,
        directory: directory.clone(),
        remove_directory: false,
        operation: Some(prepared.operation.clone()),
    }));
    let worker_cleanup = cleanup.clone();
    let database = prepared.database.clone();
    let operation = prepared.operation.clone();
    let record = prepared.record.clone();
    let root = prepared.root;
    let project = args.project_root.clone();
    let config = args.config_path;
    let preparation = tokio::task::spawn_blocking(move || {
        let root = std::fs::canonicalize(root).map_err(failure)?;
        if !project.is_absolute() {
            return Err(failure("Agent project_root must be absolute"));
        }
        let project = std::fs::canonicalize(project).map_err(failure)?;
        if !project.is_dir() || !project.starts_with(&root) {
            return Err(failure(
                "Agent project_root outside actual parent work scope",
            ));
        }
        let existing = database.agent_by_id(&record.id).is_ok();
        if !existing {
            std::fs::create_dir(&directory).map_err(failure)?;
            worker_cleanup.lock().unwrap().remove_directory = true;
        } else if !directory.is_dir() {
            std::fs::create_dir(&directory).map_err(failure)?;
        }
        let real = std::fs::canonicalize(&directory).map_err(failure)?;
        if real != directory {
            return Err(failure("Agent persistence directory is a symlink"));
        }
        let settings = directory.join("setting.toml");
        let defaults = if settings.exists() {
            String::new()
        } else {
            std::fs::read_to_string(config).map_err(failure)?
        };
        let storage =
            storage_core::open_storage(&directory.join("state.sqlite"), &settings, &defaults)
                .map_err(failure)?;
        worker_cleanup.lock().unwrap().storage = Some(storage.clone());
        if let Err(commit_error) = database.commit_agent(&operation, record.clone()) {
            match database.agent_by_id(&record.id) {
                Ok(_) => worker_cleanup.lock().unwrap().remove_directory = false,
                Err(error) if error.kind == workspace_core::WorkspaceErrorKind::Missing => {}
                Err(_) => worker_cleanup.lock().unwrap().remove_directory = false,
            }
            return Err(failure(commit_error));
        }
        worker_cleanup.lock().unwrap().remove_directory = false;
        Ok((
            storage,
            project,
            format!(
                "{}/{}",
                record.parent_node.trim_end_matches('/'),
                record.runtime_name
            ),
        ))
    })
    .await
    .map_err(failure)
    .and_then(|r| r);
    let result = async {
        let (storage, project_root, address) = preparation?;
        cleanup
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending_handoff = Some(PendingAgentHandoff {
            world: Arc::downgrade(&sender),
            address: address.clone(),
            parent: prepared.parent,
            task_id: prepared.task_id.clone(),
            storage: storage.clone(),
        });
        let (tx, rx) = oneshot::channel();
        let checked = identity.clone();
        let task = prepared.creator.clone();
        let task_id = prepared.task_id.clone();
        let ui_path = args.ui_path;
        world(sender.clone(), move |world| {
            validate_execution(world, &checked)?;
            if task.cancel_token().is_cancelled() {
                return Err(failure("creator cancelled"));
            }
            world.send_event(agent_core::StartAgent {
                workspace: checked.workspace,
                parent: checked.source,
                task_id,
                address: address.clone(),
                project_root,
                ui_path,
                persistence: storage,
                reply: Arc::new(Mutex::new(Some(tx))),
            });
            Ok(address)
        })
        .await?;
        let handle = rx.await.map_err(failure)?.map_err(failure)?;
        {
            let mut cleanup = cleanup.lock().unwrap_or_else(|e| e.into_inner());
            cleanup.agent = Some(handle.clone());
            cleanup.pending_handoff = None;
        }
        // Agent System already bound this exact handle before replying.
        if identity.cancel.is_cancelled() {
            handle.close().await.map_err(failure)?;
            return Err(failure("Agent creator cancelled"));
        }
        let entity = handle.entity();
        let execution = identity.instance.clone();
        let expected_task = prepared.creator.clone();
        let parent = prepared.parent;
        let info = world(sender.clone(), move |world| {
            let agent = world
                .get_component::<agent_core::Agent>(entity)
                .ok_or_else(|| failure("created Agent missing"))?;
            if agent.parent() != parent || agent.task_id() != invocation.task_id {
                return Err(failure("created Agent binding mismatch"));
            }
            let storage = world
                .get_component::<storage_core::AgentStorage>(entity)
                .cloned()
                .ok_or_else(|| failure("created storage missing"))?;
            let registry = world.get_resource::<ExecutionBindings>().unwrap();
            registry
                .diagnostics
                .lock()
                .unwrap()
                .entry(execution)
                .or_default()
                .insert(
                    next(),
                    MonitoredAgent {
                        entity,
                        session: agent.instance().into(),
                        storage,
                    },
                );
            let address = world
                .get_component::<address::Address>(entity)
                .ok_or_else(|| failure("Agent address missing"))?
                .as_str();
            if expected_task.cancel_token().is_cancelled() {
                return Err(failure("creator cancelled during handoff"));
            }
            Ok(json!({"address":address}))
        })
        .await;
        if info.is_err() {
            handle.close().await.map_err(failure)?;
        }
        info
    }
    .await;
    if result.is_err() {
        if let Err(close) = compensate(cleanup.clone()).await {
            let retry = cleanup.clone();
            prepared
                .creator
                .retain_cleanup(Arc::new(move || {
                    let retry = retry.clone();
                    Box::pin(compensate(retry))
                }))
                .map_err(failure)?;
            return Err(failure(format!(
                "Agent creation failed; storage cleanup retained: {close}"
            )));
        }
    } else {
        cleanup.lock().unwrap().operation = None;
    }
    result
}
pub(crate) async fn finish_task(
    sender: Arc<WorldClosureSender>,
    handle: tool_core::ToolTaskHandle,
) -> Result<(), tool_core::ToolError> {
    let execution = handle.execution_id().to_owned();
    let mut pending = world(sender.clone(), move |world| {
        Ok(world
            .get_resource::<ExecutionBindings>()
            .unwrap()
            .creating
            .lock()
            .unwrap()
            .remove(&execution)
            .unwrap_or_default())
    })
    .await
    .map_err(tool_failure)?;
    for watch in &mut pending {
        while !*watch.borrow_and_update() {
            watch.changed().await.map_err(tool_failure)?;
        }
    }
    let execution = handle.execution_id().to_owned();
    world(sender, move |world| {
        let state = world.get_resource::<ExecutionBindings>().unwrap();
        state.diagnostics.lock().unwrap().remove(&execution);
        state.invocations.lock().unwrap().remove(&execution);
        Ok(())
    })
    .await
    .map_err(tool_failure)?;
    Ok(())
}
async fn finish_execution(
    sender: Arc<WorldClosureSender>,
    identity: LuaExecutionIdentity,
    _diagnostic: Value,
) -> Result<Value, LuaRuntimeError> {
    let checked = identity.clone();
    let handle = world(sender.clone(), move |world| {
        creator_task(world, checked.source, &checked.instance).map(|(_, h)| h)
    })
    .await?;
    finish_task(sender, handle.clone()).await.map_err(failure)?;
    handle.close_registrations();
    handle.finish_children().await.map_err(failure)?;
    handle.retry_cleanups().await.map_err(failure)?;
    Ok(json!({}))
}
