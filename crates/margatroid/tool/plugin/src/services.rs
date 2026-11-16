//! Host services shared by automatic Lua tasks and tool worker requests.
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::{App, World};
use lua_runtime_core::{LuaExecutionIdentity, LuaRuntimeError, LuaRuntimeErrorKind, LuaServices};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;
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
  "workspace_agents"=>world(sender,move|world|{validate_execution(world,&identity)?;Ok(json!(world.query_with::<agent_core::Agent>().result().into_iter().filter_map(|entity|{let agent=world.get_component::<agent_core::Agent>(entity)?;if agent.workspace()!=identity.workspace{return None;}Some(json!({"name":agent.name(),"address":world.get_component::<address::Address>(entity)?.as_str(),"state":format!("{:?}",agent.state()).to_lowercase()}))}).collect::<Vec<_>>()))}).await,
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
   let value=if method=="task_send"{arguments.get("target").cloned().ok_or_else(||failure("task target missing"))?}else{arguments.clone()};
   let target=if let Some(id)=value.as_str(){tool_resource_core::TaskReference{agent:None,task_id:id.into()}}else{serde_json::from_value::<tool_resource_core::TaskReference>(value).map_err(failure)?};
   let source=identity.source;let method_copy=method.clone();let target_handle=world(sender,move|world|{validate_execution(world,&identity)?;
    let entity=if let Some(address)=target.agent{let address=address::Address::new(&address).map_err(failure)?;world.query_with::<address::Address>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&address)).ok_or_else(||failure("task Agent missing"))?}else{source};
    if super::handler::workspace_of(world,entity).map_err(failure)?!=identity.workspace{return Err(failure("task outside Workspace"));}
    let handle=world.get_component::<tool_core::ToolTasks>(entity).ok_or_else(||failure("tasks missing"))?.get(&target.task_id).map_err(failure)?;
    if method_copy=="task_finish"&&entity==source&&target.task_id==identity.instance{handle.cancel();identity.cancel.cancel();return Err(failure("execution finished"));}Ok(handle)
   }).await?;
   match method.as_str(){"task_read_output"=>serde_json::to_value(target_handle.read().map_err(failure)?).map_err(failure),"task_send"=>{target_handle.send(arguments.get("value").cloned().unwrap_or(Value::Null)).map_err(failure)?;Ok(Value::Null)},_=>{target_handle.finish().await.map_err(failure)?;Ok(Value::Null)}}
  }
  "execution_log"=>{
    let mut record:storage_core::AgentLogRecord=serde_json::from_value(arguments).map_err(failure)?;
    record.source=if identity.instance.starts_with("task-instance:"){storage_core::AgentLogSource::Tool}else{storage_core::AgentLogSource::Driver};
    let mut trusted=record.context_json.take().unwrap_or_else(||json!({}));
    let values=trusted.as_object_mut().ok_or_else(||failure("log context must be object"))?;
    values.insert("instance".into(),json!(identity.instance));values.insert("source_address".into(),json!(identity.address));record.context_json=Some(trusted);
    let agents=world(sender,move|world|{validate_execution(world,&identity)?;let agents=world.get_resource::<ExecutionBindings>().unwrap().0.lock().unwrap().get(&identity.instance).map(|agents|agents.values().cloned().collect::<Vec<_>>()).unwrap_or_default();Ok(agents.into_iter().map(|agent|{let context=bound_context(world,&agent);(agent,context)}).collect::<Vec<_>>())}).await?;
    let count=agents.len();for (agent,context) in agents{let mut record=record.clone();let mut full=record.context_json.take().unwrap_or_else(||json!({}));full.as_object_mut().ok_or_else(||failure("log context must be object"))?.extend(context.as_object().unwrap().clone());record.context_json=Some(full);tokio::task::spawn_blocking(move||storage_core::append_agent_log(&agent.storage,record)).await.map_err(failure)?.map_err(failure)?;}Ok(json!(count))
   }
  "agent_stop"=>{
    let target=arguments.as_str().ok_or_else(||failure("agent.stop requires Agent address"))?.to_owned();let(tx,rx)=oneshot::channel();
    world(sender,move|world|{validate_execution(world,&identity)?;let parsed=address::Address::new(&target).map_err(failure)?;
     let entity=world.query_with::<agent_core::Agent>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&parsed)).ok_or_else(||failure("stop Agent missing"))?;
     if world.get_component::<agent_core::Agent>(entity).unwrap().workspace()!=identity.workspace{return Err(failure("stop target outside Workspace"));}
     world.send_event(agent_core::StopAgent{id:format!("lua-stop:{}",next()),agent:entity,reply:Arc::new(std::sync::Mutex::new(Some(tx)))});
     // Stop may cancel the caller's own tool task: supervise the original reply externally.
     async_runtime_plugin::WorldAsyncExt::send_async_closure(world,RuntimePlugin::UPDATE,move||async move{match rx.await{Ok(Ok(_))=>{},other=>tracing::error!(result=?other,"injected Agent stop failed")};Ok::<(),async_runtime_plugin::AsyncTaskError>(())});Ok(())
    }).await?;Ok(json!({}))
   }
  "agent_create"=>{
   let owner=arguments.get("owner").and_then(Value::as_str).ok_or_else(||failure("Agent owner address required"))?.to_owned();
   let name=arguments.get("name").and_then(Value::as_str).ok_or_else(||failure("Agent name required"))?.to_owned();
   let config_path=arguments.get("config_path").and_then(Value::as_str).ok_or_else(||failure("Agent config_path required"))?.to_owned();
    let ui_path=arguments.get("ui_path").and_then(Value::as_str).ok_or_else(||failure("Agent ui_path required"))?.to_owned();
    if arguments.as_object().is_none_or(|fields|fields.keys().any(|key|!matches!(key.as_str(),"owner"|"name"|"config_path"|"ui_path"))){return Err(failure("unknown Agent creation parameter"));}
   let cancel=identity.cancel.clone();let(tx,rx)=oneshot::channel();
   let sender_for_info=sender.clone();let execution=identity.instance.clone();
   world(sender,move|world|{validate_execution(world,&identity)?;
    let requested=address::Address::new(&owner).map_err(failure)?;
    let owner=world.query_with::<address::Address>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&requested)).ok_or_else(||failure("Agent owner missing"))?;
    let root=world.get_component::<workspace_core::Workspace>(identity.workspace).ok_or_else(||failure("Workspace missing"))?;
    if root.state==workspace_core::WorkspaceState::Stopping{return Err(failure("Workspace closing"));}
    let parent=if owner==identity.workspace{root.definition_path.parent().unwrap().to_path_buf()}else{
     let parent=world.get_component::<agent_core::Agent>(owner).ok_or_else(||failure("owner is not Workspace or Agent"))?;
     if parent.workspace()!=identity.workspace||matches!(parent.state(),agent_core::AgentState::Stopping|agent_core::AgentState::Error){return Err(failure("parent Agent unavailable"));}
     world.get_component::<agent_core::AgentResources>(owner).and_then(|source|source.parameters.settings_path.parent()).ok_or_else(||failure("parent Agent resources missing"))?.to_path_buf()
    };
    if name.is_empty()||matches!(name.as_str(),"."|"..")||name.chars().any(|c|matches!(c,'/'|'\\'|':')||c.is_control()){return Err(failure("invalid Agent name"));}
    let home=parent.join("agents").join(&name);let parameters=agent_core::AgentStartParameters{initial_settings_path:config_path.into(),ui_path:ui_path.into(),project_root:root.project_root.clone(),home_root:home.clone(),database_path:home.join("state.sqlite"),settings_path:home.join("setting.toml")};
    world.send_event(agent_core::StartAgent{owner,id:format!("lua-create:{}",next()),workspace:identity.workspace,name,parameters,reply:Arc::new(std::sync::Mutex::new(Some(tx)))});Ok(())
   }).await?;
   let entity=rx.await.map_err(failure)?.map_err(failure)?;
   let info=world(sender_for_info,move|world|{let agent=world.get_component::<agent_core::Agent>(entity).ok_or_else(||failure("created Agent missing"))?;let info=json!({"address":world.get_component::<address::Address>(entity).unwrap().as_str(),"name":agent.name(),"workspace":world.get_component::<address::Address>(agent.workspace()).unwrap().as_str()});bind_agent(world,&execution,entity)?;Ok(info)}).await?;
   if cancel.is_cancelled(){return Err(failure("Agent creation caller cancelled after creation"));}Ok(info)
  }
  "wait"=>{
   let target=arguments.as_str().ok_or_else(||failure("wait requires Agent address"))?.to_owned();let cancel=identity.cancel.clone();
   let slot=world(sender,move|world|{validate_execution(world,&identity)?;let address=address::Address::new(&target).map_err(failure)?;let entity=world.query_with::<address::Address>().result().into_iter().find(|entity|world.get_component::<address::Address>(*entity)==Some(&address)).ok_or_else(||failure("wait Agent missing"))?;
    if world.get_component::<agent_core::Agent>(entity).is_none_or(|agent|agent.workspace()!=identity.workspace){return Err(failure("wait target outside Workspace"));}
    acknowledge_execution(world,&identity);
     Ok(world.get_component::<lua_runtime_core::AgentWaitSlot>(entity).ok_or_else(||failure("Agent wait slot missing"))?.0.clone())
   }).await?;
   tokio::select!{biased;_=cancel.cancelled()=>Err(failure("wait cancelled")),result=lua_runtime_core::DriverWakeSlot::wait(slot)=>{result?;Ok(Value::Null)}}
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
#[derive(Default)]
struct ExecutionBindings(
    Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, std::collections::BTreeMap<u64, MonitoredAgent>>,
        >,
    >,
);
impl core_plugin::Resource for ExecutionBindings {}
fn bind_agent(
    world: &mut World,
    instance: &str,
    entity: core_plugin::Entity,
) -> Result<(), LuaRuntimeError> {
    let session = world
        .get_component::<agent_core::AgentSession>(entity)
        .ok_or_else(|| failure("Agent session missing"))?
        .0
        .clone();
    let storage = world
        .get_component::<storage_core::AgentStorage>(entity)
        .cloned()
        .ok_or_else(|| failure("Agent log storage missing"))?;
    let registry = world.get_resource::<ExecutionBindings>().unwrap().0.clone();
    let id = next();
    registry
        .lock()
        .unwrap()
        .entry(instance.into())
        .or_default()
        .insert(
            id,
            MonitoredAgent {
                entity,
                session,
                storage,
            },
        );
    let weak = Arc::downgrade(&registry);
    let instance = instance.to_owned();
    world.insert_component(
        entity,
        lua_runtime_core::LuaAgentMonitor::new(move || {
            if let Some(registry) = weak.upgrade() {
                let mut bindings = registry.lock().unwrap();
                if let Some(agents) = bindings.get_mut(&instance) {
                    agents.remove(&id);
                    if agents.is_empty() {
                        bindings.remove(&instance);
                    }
                }
            }
        }),
    );
    Ok(())
}
fn bound_context(world: &World, agent: &MonitoredAgent) -> Value {
    let current = world.get_component::<agent_core::Agent>(agent.entity);
    json!({"agent":world.get_component::<address::Address>(agent.entity).map(|address|address.as_str()),"session":agent.session,"turn":current.and_then(|agent|agent.turn().clone()),"workspace":current.and_then(|agent|world.get_component::<address::Address>(agent.workspace()).map(|address|address.as_str().to_owned()))})
}
async fn finish_execution(
    sender: Arc<WorldClosureSender>,
    identity: LuaExecutionIdentity,
    diagnostic: Value,
) -> Result<Value, LuaRuntimeError> {
    let instance = identity.instance.clone();
    let agents = world(sender.clone(), move |world| {
        let agents = world
            .get_resource::<ExecutionBindings>()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .remove(&instance)
            .unwrap_or_default();
        Ok(agents
            .into_values()
            .map(|agent| {
                let context = bound_context(world, &agent);
                (agent, context)
            })
            .collect::<Vec<_>>())
    })
    .await?;
    if diagnostic.is_null() {
        return Ok(json!({}));
    }
    let mut log_failures = Vec::new();
    for (monitored, mut context) in agents {
        let storage = monitored.storage.clone();
        let details = diagnostic.clone();
        let source = identity.address.clone();
        let instance = identity.instance.clone();
        context["instance"] = json!(instance);
        context["source_address"] = json!(source);
        let log_source = if identity.instance.starts_with("task-instance:") {
            storage_core::AgentLogSource::Tool
        } else {
            storage_core::AgentLogSource::Driver
        };
        let persisted = tokio::task::spawn_blocking(move || {
            storage_core::append_agent_log(
                &storage,
                storage_core::AgentLogRecord {
                    created_at_ms: None,
                    level: storage_core::AgentLogLevel::Error,
                    source: log_source,
                    stage: "execute".into(),
                    message: details["message"]
                        .as_str()
                        .unwrap_or("Lua execution failed")
                        .into(),
                    details_json: details.get("details").cloned(),
                    context_json: Some(context),
                    origin: details["origin"].as_str().map(str::to_owned),
                    raw_error: details["raw_error"].as_str().map(str::to_owned),
                    traceback: details["traceback"].as_str().map(str::to_owned),
                },
            )
        })
        .await
        .map_err(failure)
        .and_then(|value| value.map_err(failure));
        if let Err(error) = persisted {
            log_failures.push(error.to_string());
        }
        let message = diagnostic["message"]
            .as_str()
            .unwrap_or("Lua execution failed")
            .to_owned();
        world(sender.clone(), move |world| {
            if world
                .get_component::<agent_core::AgentSession>(monitored.entity)
                .is_some_and(|session| session.0 == monitored.session)
                && world
                    .get_component::<agent_core::Agent>(monitored.entity)
                    .is_some_and(|agent| {
                        matches!(
                            agent.state(),
                            agent_core::AgentState::Starting | agent_core::AgentState::Running
                        )
                    })
            {
                world.send_event(agent_core::AgentException {
                    id: format!("lua-fault:{}", next()),
                    agent: monitored.entity,
                    error: agent_core::AgentError::new(
                        agent_core::AgentErrorKind::StartFailed,
                        message,
                    ),
                });
            }
            Ok(())
        })
        .await?;
    }
    if log_failures.is_empty() {
        Ok(json!({}))
    } else {
        Err(failure(log_failures.join("; ")))
    }
}
