use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::{Duration, Instant};

use app_runtime_plugin::RuntimeEventSender;
use core_plugin::Resource;
use margatroid_types::LuaVmId;
use tokio::sync::oneshot;

use crate::error::LuaRuntimeError;
use crate::events::{
    LuaRuntimeCancelRequest, LuaRuntimeRequest, LuaVmMessage, LuaVmMessageReceiveRequest,
};

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
    pub(crate) fn cancel(&self) {
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
pub(crate) struct LuaRuntimeState {
    pub(crate) next_vm: AtomicU64,
    pub(crate) sessions: HashMap<LuaVmId, LuaVmSession>,
    pub(crate) requests: HashMap<String, CancellationToken>,
    pub(crate) owners: HashMap<String, String>,
    pub(crate) mailboxes: HashMap<LuaVmId, VecDeque<LuaValue>>,
    pub(crate) receives: HashMap<LuaVmId, VecDeque<LuaVmMessageReceiveRequest>>,
}
impl Resource for LuaRuntimeState {}

#[derive(Clone)]
pub struct LuaRuntimeHandle {
    pub(crate) events: RuntimeEventSender,
    pub(crate) environments: Arc<RwLock<LuaEnvironmentRegistry>>,
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
