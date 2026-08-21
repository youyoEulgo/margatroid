use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::{Duration, Instant};

use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Event, Plugin, Resource, World};
use margatroid_types::LuaVmId;
use mlua::{Lua, LuaOptions, StdLib, Value as MlValue};
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq)]
pub enum LuaValue {
    Nil,
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Array(Vec<LuaValue>),
    Object(BTreeMap<String, LuaValue>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LuaRuntimeError {
    RuntimeClosed,
    InvalidRequest(String),
    SourceTooLarge,
    ResultTooLarge,
    ProviderAlreadyRegistered(String),
    EnvironmentProviderNotFound(String),
    EnvironmentConflict(String),
    EnvironmentFailed(String),
    SchedulerUnavailable,
    Timeout,
    Cancelled,
    VmCreationFailed(String),
    VmExecutionFailed(String),
}
impl fmt::Display for LuaRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for LuaRuntimeError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaVmOwner {
    pub owner_id: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaProgram {
    pub source: String,
    pub origin: String,
    pub entry: Option<String>,
    pub libraries: LuaStandardLibraries,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaStandardLibraries {
    Safe,
    Full,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaEnvironmentContext {
    pub request_id: String,
    pub owner: LuaVmOwner,
    pub values: BTreeMap<String, LuaValue>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaEnvironment {
    pub globals: Vec<LuaGlobalBinding>,
    pub modules: Vec<LuaModuleBinding>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaGlobalBinding {
    pub name: String,
    pub binding: LuaBindingValue,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaModuleBinding {
    pub name: String,
    pub exports: BTreeMap<String, LuaBindingValue>,
}
#[derive(Clone)]
pub enum LuaBindingValue {
    Value(LuaValue),
    Function(Arc<dyn LuaHostFunction>),
}
impl fmt::Debug for LuaBindingValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(v) => f.debug_tuple("Value").field(v).finish(),
            Self::Function(_) => f.write_str("Function(..)"),
        }
    }
}
impl PartialEq for LuaBindingValue {
    fn eq(&self, other: &Self) -> bool {
        matches!((self, other), (Self::Value(a), Self::Value(b)) if a == b)
    }
}
pub type HostFuture =
    Pin<Box<dyn Future<Output = Result<LuaValue, LuaRuntimeError>> + Send + 'static>>;
pub trait LuaHostFunction: Send + Sync + 'static {
    fn call(
        &self,
        arguments: LuaValue,
        context: LuaEnvironmentContext,
        cancel: CancellationToken,
    ) -> HostFuture;
}
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaScheduler {
    LongRunning,
    WorkerPool,
    DedicatedThread,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaVmState {
    Starting,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct LuaRuntimeReply {
    sender: Arc<Mutex<Option<oneshot::Sender<LuaRuntimeResult>>>>,
}
impl LuaRuntimeReply {
    pub fn new(sender: oneshot::Sender<LuaRuntimeResult>) -> Self {
        Self {
            sender: Arc::new(Mutex::new(Some(sender))),
        }
    }
    pub fn take(&self) -> Option<oneshot::Sender<LuaRuntimeResult>> {
        self.sender.lock().ok()?.take()
    }
    pub fn fail(&self, error: LuaRuntimeError) {
        if let Some(sender) = self.take() {
            let _ = sender.send(LuaRuntimeResult::Failed { error });
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum LuaRuntimeResult {
    Completed { value: LuaValue },
    Failed { error: LuaRuntimeError },
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct LuaRuntimeRequest {
    pub request_id: String,
    pub owner: LuaVmOwner,
    pub program: LuaProgram,
    pub context: LuaEnvironmentContext,
    pub providers: Vec<String>,
    pub scheduler: LuaScheduler,
    pub deadline: Option<Instant>,
    pub reply: LuaRuntimeReply,
}
impl Event for LuaRuntimeRequest {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaRuntimeCancelRequest {
    pub request_id: String,
    pub vm_id: Option<LuaVmId>,
}
impl Event for LuaRuntimeCancelRequest {}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaVmMessage {
    pub vm_id: LuaVmId,
    pub value: LuaValue,
}
impl Event for LuaVmMessage {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaVmMessageReceiveRequest {
    pub id: String,
    pub vm_id: LuaVmId,
}
impl Event for LuaVmMessageReceiveRequest {}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaVmMessageReceived {
    pub id: String,
    pub vm_id: LuaVmId,
    pub result: Result<LuaValue, LuaRuntimeError>,
}
impl Event for LuaVmMessageReceived {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaVmStarted {
    pub request_id: String,
    pub vm_id: LuaVmId,
    pub owner: LuaVmOwner,
}
impl Event for LuaVmStarted {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaRuntimeTaskFinished {
    pub request_id: String,
    pub vm_id: Option<LuaVmId>,
    pub owner: LuaVmOwner,
    pub state: LuaVmState,
    pub error: Option<LuaRuntimeError>,
}
impl Event for LuaRuntimeTaskFinished {}

pub trait LuaEnvironmentProvider: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn provide(&self, context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>;
}
#[derive(Default)]
pub struct LuaEnvironmentRegistry {
    providers: BTreeMap<String, Box<dyn LuaEnvironmentProvider>>,
}
impl LuaEnvironmentRegistry {
    pub fn register(
        &mut self,
        provider: Box<dyn LuaEnvironmentProvider>,
    ) -> Result<(), LuaRuntimeError> {
        let name = provider.name().to_owned();
        if name.is_empty() {
            return Err(LuaRuntimeError::InvalidRequest(
                "provider name is empty".into(),
            ));
        }
        if self.providers.contains_key(&name) {
            return Err(LuaRuntimeError::ProviderAlreadyRegistered(name));
        }
        self.providers.insert(name, provider);
        Ok(())
    }
    pub fn collect(
        &self,
        names: &[String],
        context: &LuaEnvironmentContext,
    ) -> Result<LuaEnvironment, LuaRuntimeError> {
        let mut env = LuaEnvironment {
            globals: Vec::new(),
            modules: Vec::new(),
        };
        let mut globals = std::collections::HashSet::new();
        let mut modules = std::collections::HashSet::new();
        for name in names {
            let provider = self
                .providers
                .get(name)
                .ok_or_else(|| LuaRuntimeError::EnvironmentProviderNotFound(name.clone()))?;
            let part = provider
                .provide(context)
                .map_err(|e| LuaRuntimeError::EnvironmentFailed(e.to_string()))?;
            for binding in part.globals {
                if !globals.insert(binding.name.clone()) {
                    return Err(LuaRuntimeError::EnvironmentConflict(binding.name));
                }
                env.globals.push(binding);
            }
            for module in part.modules {
                if !modules.insert(module.name.clone()) {
                    return Err(LuaRuntimeError::EnvironmentConflict(module.name));
                }
                env.modules.push(module);
            }
        }
        Ok(env)
    }
}

#[derive(Clone, Debug)]
pub struct LuaVmSession {
    pub vm_id: LuaVmId,
    pub owner: LuaVmOwner,
    pub state: LuaVmState,
    pub created_at: Instant,
    pub last_activity: Instant,
}
#[derive(Clone, Debug)]
pub struct LuaRuntimeConfig {
    pub max_source_bytes: usize,
    pub max_result_bytes: usize,
    pub default_timeout: Duration,
    pub queue_capacity: usize,
    pub worker_count: usize,
}
impl Resource for LuaRuntimeConfig {}
impl Default for LuaRuntimeConfig {
    fn default() -> Self {
        Self {
            max_source_bytes: 1024 * 1024,
            max_result_bytes: 4 * 1024 * 1024,
            default_timeout: Duration::from_secs(30),
            queue_capacity: 128,
            worker_count: 4,
        }
    }
}
#[derive(Default)]
struct LuaRuntimeState {
    next_vm: AtomicU64,
    sessions: HashMap<LuaVmId, LuaVmSession>,
    requests: HashMap<String, CancellationToken>,
    owners: HashMap<String, String>,
    mailboxes: HashMap<LuaVmId, VecDeque<LuaValue>>,
    receives: HashMap<LuaVmId, VecDeque<LuaVmMessageReceiveRequest>>,
}
impl Resource for LuaRuntimeState {}

#[derive(Clone)]
pub struct LuaRuntimeHandle {
    events: RuntimeEventSender,
    environments: Arc<RwLock<LuaEnvironmentRegistry>>,
}
impl Resource for LuaRuntimeHandle {}
impl LuaRuntimeHandle {
    pub fn submit(&self, request: LuaRuntimeRequest) -> Result<(), LuaRuntimeError> {
        self.events.send_event(request);
        Ok(())
    }
    pub fn register_long_running(&self, request: LuaRuntimeRequest) -> Result<(), LuaRuntimeError> {
        if request.scheduler != LuaScheduler::LongRunning || request.owner.owner_id.is_empty() {
            return Err(LuaRuntimeError::InvalidRequest(
                "long-running request is invalid".into(),
            ));
        }
        self.submit(request)
    }
    pub fn stop_long_running(&self, owner_id: &str) -> Result<(), LuaRuntimeError> {
        self.events.send_event(LuaRuntimeCancelRequest {
            request_id: owner_id.to_owned(),
            vm_id: None,
        });
        Ok(())
    }
    pub fn send_message(&self, vm_id: LuaVmId, value: LuaValue) -> Result<(), LuaRuntimeError> {
        self.events.send_event(LuaVmMessage { vm_id, value });
        Ok(())
    }
    pub fn receive_message(&self, id: String, vm_id: LuaVmId) -> Result<(), LuaRuntimeError> {
        self.events
            .send_event(LuaVmMessageReceiveRequest { id, vm_id });
        Ok(())
    }
    pub fn cancel(&self, request_id: &str) -> Result<(), LuaRuntimeError> {
        self.events.send_event(LuaRuntimeCancelRequest {
            request_id: request_id.to_owned(),
            vm_id: None,
        });
        Ok(())
    }
    pub fn register_provider(
        &self,
        provider: Box<dyn LuaEnvironmentProvider>,
    ) -> Result<(), LuaRuntimeError> {
        self.environments
            .write()
            .map_err(|_| LuaRuntimeError::RuntimeClosed)?
            .register(provider)
    }
}

pub struct LuaRuntimePlugin {
    schedule: String,
    config: LuaRuntimeConfig,
}
impl LuaRuntimePlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.to_owned(),
            config: LuaRuntimeConfig::default(),
        }
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
    pub fn with_config(mut self, config: LuaRuntimeConfig) -> Self {
        self.config = config;
        self
    }
}
impl Default for LuaRuntimePlugin {
    fn default() -> Self {
        Self::new()
    }
}
pub struct LuaRuntimePluginInstalled;
impl Resource for LuaRuntimePluginInstalled {}
impl Plugin for LuaRuntimePlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
        {
            panic!("LuaRuntimePlugin requires RuntimePlugin and AsyncRuntimePlugin");
        }
        if app.world().contains_resource::<LuaRuntimePluginInstalled>() {
            panic!("LuaRuntimePlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("LuaRuntimePlugin schedule does not exist");
        }
        let events = app.world().event_sender();
        let environments = Arc::new(RwLock::new(LuaEnvironmentRegistry::default()));
        app.world_mut().insert_resource(LuaRuntimeHandle {
            events,
            environments,
        });
        app.world_mut().insert_resource(self.config);
        app.world_mut().insert_resource(LuaRuntimeState::default());
        app.world_mut().insert_resource(LuaRuntimePluginInstalled);
        app.add_system(&self.schedule, lua_runtime_request_system)
            .add_system(&self.schedule, lua_runtime_cancel_system)
            .add_system(&self.schedule, lua_vm_message_system)
            .add_system(&self.schedule, lua_vm_receive_system)
            .add_system(&self.schedule, lua_runtime_result_system);
    }
}

fn lua_runtime_request_system(world: &mut World) {
    let requests = world
        .event_reader::<LuaRuntimeRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let max_source = world
        .get_resource::<LuaRuntimeConfig>()
        .map_or(1024 * 1024, |c| c.max_source_bytes);
    let runtime = world.get_resource::<LuaRuntimeHandle>().cloned();
    for request in requests {
        if request.request_id.is_empty() {
            request.reply.fail(LuaRuntimeError::InvalidRequest(
                "request id is empty".into(),
            ));
            world.emit_event(LuaRuntimeTaskFinished {
                request_id: request.request_id,
                vm_id: None,
                owner: request.owner,
                state: LuaVmState::Failed,
                error: Some(LuaRuntimeError::InvalidRequest(
                    "request id is empty".into(),
                )),
            });
            continue;
        }
        let duplicate = world
            .get_resource::<LuaRuntimeState>()
            .is_some_and(|state| state.requests.contains_key(&request.request_id));
        if duplicate {
            let error = LuaRuntimeError::InvalidRequest("request id is already running".into());
            request.reply.fail(error.clone());
            world.emit_event(LuaRuntimeTaskFinished {
                request_id: request.request_id,
                vm_id: None,
                owner: request.owner,
                state: LuaVmState::Failed,
                error: Some(error),
            });
            continue;
        }
        if request.program.source.len() > max_source {
            request.reply.fail(LuaRuntimeError::SourceTooLarge);
            world.emit_event(LuaRuntimeTaskFinished {
                request_id: request.request_id,
                vm_id: None,
                owner: request.owner,
                state: LuaVmState::Failed,
                error: Some(LuaRuntimeError::SourceTooLarge),
            });
            continue;
        }
        let (vm_id, cancel) = {
            let Some(state) = world.get_resource_mut::<LuaRuntimeState>() else {
                request.reply.fail(LuaRuntimeError::RuntimeClosed);
                continue;
            };
            let vm_id = LuaVmId(state.next_vm.fetch_add(1, Ordering::Relaxed) + 1);
            let now = Instant::now();
            state.sessions.insert(
                vm_id,
                LuaVmSession {
                    vm_id,
                    owner: request.owner.clone(),
                    state: LuaVmState::Running,
                    created_at: now,
                    last_activity: now,
                },
            );
            state.mailboxes.entry(vm_id).or_default();
            let cancel = CancellationToken::default();
            state
                .requests
                .insert(request.request_id.clone(), cancel.clone());
            state
                .owners
                .insert(request.request_id.clone(), request.owner.owner_id.clone());
            (vm_id, cancel)
        };
        let owner = request.owner.clone();
        let request_id = request.request_id.clone();
        let reply = request.reply.clone();
        let events = runtime.as_ref().map(|h| h.events.clone());
        let registry = runtime.as_ref().map(|h| Arc::clone(&h.environments));
        if request.scheduler == LuaScheduler::LongRunning {
            if let Some(sender) = &events {
                sender.send_event(LuaVmStarted {
                    request_id: request_id.clone(),
                    vm_id,
                    owner: owner.clone(),
                });
            }
        }
        let program = request.program;
        let context = request.context;
        let providers = request.providers;
        let deadline = request.deadline;
        let max_result = world
            .get_resource::<LuaRuntimeConfig>()
            .map_or(4 * 1024 * 1024, |config| config.max_result_bytes);
        std::thread::spawn(move || {
            let result = if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                Err(LuaRuntimeError::Timeout)
            } else {
                execute_lua(program, context, providers, registry, cancel.clone()).and_then(
                    |value| {
                        (lua_value_size(&value) <= max_result)
                            .then_some(value)
                            .ok_or(LuaRuntimeError::ResultTooLarge)
                    },
                )
            };
            if cancel.is_cancelled() {
                if let Some(sender) = events {
                    sender.send_event(LuaRuntimeTaskFinished {
                        request_id,
                        vm_id: Some(vm_id),
                        owner,
                        state: LuaVmState::Cancelled,
                        error: None,
                    });
                }
                let _ = reply.take().map(|s| s.send(LuaRuntimeResult::Cancelled));
            } else {
                let state = if result.is_ok() {
                    LuaVmState::Completed
                } else {
                    LuaVmState::Failed
                };
                if let Some(sender) = events {
                    sender.send_event(LuaRuntimeTaskFinished {
                        request_id,
                        vm_id: Some(vm_id),
                        owner,
                        state,
                        error: result.as_ref().err().cloned(),
                    });
                }
                let _ = reply.take().map(|s| {
                    s.send(
                        result
                            .map(|value| LuaRuntimeResult::Completed { value })
                            .unwrap_or_else(|error| LuaRuntimeResult::Failed { error }),
                    )
                });
            }
        });
    }
}

fn lua_runtime_cancel_system(world: &mut World) {
    let requests = world
        .event_reader::<LuaRuntimeCancelRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if let Some(state) = world.get_resource::<LuaRuntimeState>() {
        for request in requests {
            if let Some(token) = state.requests.get(&request.request_id) {
                token.cancel();
            }
            for (request_id, owner_id) in &state.owners {
                if owner_id == &request.request_id {
                    if let Some(token) = state.requests.get(request_id) {
                        token.cancel();
                    }
                }
            }
        }
    }
}
fn lua_vm_message_system(world: &mut World) {
    let messages = world
        .event_reader::<LuaVmMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut responses = Vec::new();
    let Some(state) = world.get_resource_mut::<LuaRuntimeState>() else {
        return;
    };
    for message in messages {
        if let Some(waiting) = state
            .receives
            .get_mut(&message.vm_id)
            .and_then(VecDeque::pop_front)
        {
            responses.push(LuaVmMessageReceived {
                id: waiting.id,
                vm_id: message.vm_id,
                result: Ok(message.value),
            });
        } else if let Some(mailbox) = state.mailboxes.get_mut(&message.vm_id) {
            mailbox.push_back(message.value);
        }
    }
    for response in responses {
        world.emit_event(response);
    }
}
fn lua_vm_receive_system(world: &mut World) {
    let requests = world
        .event_reader::<LuaVmMessageReceiveRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut responses = Vec::new();
    let Some(state) = world.get_resource_mut::<LuaRuntimeState>() else {
        return;
    };
    for request in requests {
        match state
            .mailboxes
            .get_mut(&request.vm_id)
            .and_then(VecDeque::pop_front)
        {
            Some(value) => responses.push(LuaVmMessageReceived {
                id: request.id,
                vm_id: request.vm_id,
                result: Ok(value),
            }),
            None if state.sessions.contains_key(&request.vm_id) => {
                let receives = state.receives.entry(request.vm_id).or_default();
                if receives.is_empty() {
                    receives.push_back(request);
                } else {
                    responses.push(LuaVmMessageReceived {
                        id: request.id,
                        vm_id: request.vm_id,
                        result: Err(LuaRuntimeError::InvalidRequest(
                            "only one receive may wait on a VM mailbox".into(),
                        )),
                    });
                }
            }
            None => responses.push(LuaVmMessageReceived {
                id: request.id,
                vm_id: request.vm_id,
                result: Err(LuaRuntimeError::VmExecutionFailed(
                    "VM is not running".into(),
                )),
            }),
        }
    }
    for response in responses {
        world.emit_event(response);
    }
}

fn lua_runtime_result_system(world: &mut World) {
    let finished = world
        .event_reader::<LuaRuntimeTaskFinished>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let Some(state) = world.get_resource_mut::<LuaRuntimeState>() else {
        return;
    };
    let mut receive_failures = Vec::new();
    for event in finished {
        state.requests.remove(&event.request_id);
        state.owners.remove(&event.request_id);
        if let Some(vm_id) = event.vm_id {
            state.sessions.remove(&vm_id);
            state.mailboxes.remove(&vm_id);
            if let Some(receives) = state.receives.remove(&vm_id) {
                for receive in receives {
                    receive_failures.push(LuaVmMessageReceived {
                        id: receive.id,
                        vm_id,
                        result: Err(event
                            .error
                            .clone()
                            .unwrap_or(LuaRuntimeError::RuntimeClosed)),
                    });
                }
            }
        }
    }
    for response in receive_failures {
        world.emit_event(response);
    }
}

fn execute_lua(
    program: LuaProgram,
    context: LuaEnvironmentContext,
    providers: Vec<String>,
    registry: Option<Arc<RwLock<LuaEnvironmentRegistry>>>,
    cancellation: CancellationToken,
) -> Result<LuaValue, LuaRuntimeError> {
    let lua = match program.libraries {
        LuaStandardLibraries::Safe => Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())
            .map_err(|error| LuaRuntimeError::VmCreationFailed(error.to_string()))?,
        LuaStandardLibraries::Full => unsafe {
            Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default())
        },
    };
    if let Some(registry) = registry {
        let env = registry
            .read()
            .map_err(|_| LuaRuntimeError::RuntimeClosed)?
            .collect(&providers, &context)?;
        for binding in env.globals {
            match binding.binding {
                LuaBindingValue::Value(value) => {
                    lua.globals()
                        .set(binding.name, to_ml_value(&lua, value)?)
                        .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
                }
                LuaBindingValue::Function(function) => {
                    let context = context.clone();
                    let cancellation = cancellation.clone();
                    let callback = lua
                        .create_function(move |lua, arguments: mlua::MultiValue| {
                            let arguments = LuaValue::Array(
                                arguments
                                    .into_iter()
                                    .map(from_ml_value)
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(mlua::Error::external)?,
                            );
                            let context = context.clone();
                            let function = function.clone();
                            let cancellation = cancellation.clone();
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(mlua::Error::external)?;
                            let value = runtime
                                .block_on(function.call(arguments, context, cancellation))
                                .map_err(mlua::Error::external)?;
                            to_ml_value(lua, value).map_err(mlua::Error::external)
                        })
                        .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
                    lua.globals()
                        .set(binding.name, callback)
                        .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
                }
            }
        }
        for module in env.modules {
            let table = lua
                .create_table()
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            for (name, binding) in module.exports {
                let value = match binding {
                    LuaBindingValue::Value(value) => to_ml_value(&lua, value)?,
                    LuaBindingValue::Function(function) => {
                        let context = context.clone();
                        let cancellation = cancellation.clone();
                        MlValue::Function(
                            lua.create_function(move |lua, arguments: mlua::MultiValue| {
                                let arguments = LuaValue::Array(
                                    arguments
                                        .into_iter()
                                        .map(from_ml_value)
                                        .collect::<Result<Vec<_>, _>>()
                                        .map_err(mlua::Error::external)?,
                                );
                                let runtime = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .map_err(mlua::Error::external)?;
                                let value = runtime
                                    .block_on(function.call(
                                        arguments,
                                        context.clone(),
                                        cancellation.clone(),
                                    ))
                                    .map_err(mlua::Error::external)?;
                                to_ml_value(lua, value).map_err(mlua::Error::external)
                            })
                            .map_err(|error| {
                                LuaRuntimeError::VmExecutionFailed(error.to_string())
                            })?,
                        )
                    }
                };
                table
                    .set(name, value)
                    .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            }
            let package = lua
                .globals()
                .get::<mlua::Table>("package")
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            let loaded = package
                .get::<mlua::Table>("loaded")
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            loaded
                .set(module.name, table)
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
        }
    }
    let chunk = lua.load(&program.source);
    if cancellation.is_cancelled() {
        return Err(LuaRuntimeError::Cancelled);
    }
    let value = if let Some(entry) = program.entry {
        chunk
            .exec()
            .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
        lua.globals()
            .get::<MlValue>(entry)
            .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?
    } else {
        chunk
            .eval::<MlValue>()
            .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?
    };
    from_ml_value(value)
}

fn lua_value_size(value: &LuaValue) -> usize {
    match value {
        LuaValue::Nil => 1,
        LuaValue::Boolean(_) => 1,
        LuaValue::Integer(_) | LuaValue::Number(_) => 8,
        LuaValue::String(value) => value.len(),
        LuaValue::Array(values) => values.iter().map(lua_value_size).sum(),
        LuaValue::Object(values) => values
            .iter()
            .map(|(key, value)| key.len() + lua_value_size(value))
            .sum(),
    }
}
fn to_ml_value(lua: &Lua, value: LuaValue) -> Result<MlValue, LuaRuntimeError> {
    Ok(match value {
        LuaValue::Nil => MlValue::Nil,
        LuaValue::Boolean(v) => MlValue::Boolean(v),
        LuaValue::Integer(v) => MlValue::Integer(v),
        LuaValue::Number(v) => MlValue::Number(v),
        LuaValue::String(v) => MlValue::String(
            lua.create_string(&v)
                .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?,
        ),
        LuaValue::Array(values) => {
            let table = lua
                .create_table()
                .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
            for (index, value) in values.into_iter().enumerate() {
                table
                    .set(index + 1, to_ml_value(lua, value)?)
                    .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
            }
            MlValue::Table(table)
        }
        LuaValue::Object(values) => {
            let table = lua
                .create_table()
                .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
            for (key, value) in values {
                table
                    .set(key, to_ml_value(lua, value)?)
                    .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
            }
            MlValue::Table(table)
        }
    })
}
fn from_ml_value(value: MlValue) -> Result<LuaValue, LuaRuntimeError> {
    Ok(match value {
        MlValue::Nil => LuaValue::Nil,
        MlValue::Boolean(v) => LuaValue::Boolean(v),
        MlValue::Integer(v) => LuaValue::Integer(v),
        MlValue::Number(v) => LuaValue::Number(v),
        MlValue::String(v) => LuaValue::String(
            v.to_str()
                .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?
                .to_owned(),
        ),
        MlValue::Table(table) => {
            let mut array = Vec::new();
            let mut object = BTreeMap::new();
            for pair in table.pairs::<MlValue, MlValue>() {
                let (key, value) =
                    pair.map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?;
                match key {
                    MlValue::Integer(index) if index > 0 => {
                        if index as usize != array.len() + 1 {
                            return Err(LuaRuntimeError::VmExecutionFailed(
                                "result table is not a contiguous array".into(),
                            ));
                        }
                        array.push(from_ml_value(value)?);
                    }
                    MlValue::String(key) => {
                        object.insert(
                            key.to_str()
                                .map_err(|e| LuaRuntimeError::VmExecutionFailed(e.to_string()))?
                                .to_owned(),
                            from_ml_value(value)?,
                        );
                    }
                    _ => {
                        return Err(LuaRuntimeError::VmExecutionFailed(
                            "result table key is unsupported".into(),
                        ))
                    }
                }
            }
            if !array.is_empty() && object.is_empty() {
                LuaValue::Array(array)
            } else {
                LuaValue::Object(object)
            }
        }
        _ => {
            return Err(LuaRuntimeError::VmExecutionFailed(
                "result value is not serializable".into(),
            ))
        }
    })
}
