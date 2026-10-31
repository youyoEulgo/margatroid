use crate::{LuaRuntimeError, LuaRuntimeErrorKind};
use core_plugin::Entity;
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DriverLuaVmInstance {
    pub owner: Entity,
    pub start_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaProgram {
    pub source: String,
    pub origin: PathBuf,
    pub libraries: LuaStandardLibraries,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LuaStandardLibraries {
    #[default]
    Minimal,
    Safe,
    Full,
}

#[derive(Clone, Debug, Default)]
pub struct LuaEnvironment {
    pub globals: BTreeMap<String, LuaBinding>,
    pub modules: BTreeMap<String, LuaBinding>,
}

#[derive(Clone)]
pub enum LuaBinding {
    Value(LuaValue),
    Function(Arc<dyn LuaHostFunction>),
}

impl fmt::Debug for LuaBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(value) => formatter.debug_tuple("Value").field(value).finish(),
            Self::Function(_) => formatter.write_str("Function(..)"),
        }
    }
}

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

impl LuaValue {
    pub fn from_json(value: serde_json::Value) -> Result<Self, LuaRuntimeError> {
        Ok(match value {
            serde_json::Value::Null => Self::Nil,
            serde_json::Value::Bool(value) => Self::Boolean(value),
            serde_json::Value::Number(value) => {
                if let Some(integer) = value.as_i64() {
                    Self::Integer(integer)
                } else if value.is_u64() {
                    return Err(LuaRuntimeError::new(
                        LuaRuntimeErrorKind::InvalidRequest,
                        "JSON integer exceeds Lua integer range",
                    ));
                } else {
                    Self::Number(value.as_f64().ok_or_else(|| {
                        LuaRuntimeError::new(
                            LuaRuntimeErrorKind::InvalidRequest,
                            "invalid JSON number",
                        )
                    })?)
                }
            }
            serde_json::Value::String(value) => Self::String(value),
            serde_json::Value::Array(values) => Self::Array(
                values
                    .into_iter()
                    .map(Self::from_json)
                    .collect::<Result<_, _>>()?,
            ),
            serde_json::Value::Object(values) => Self::Object(
                values
                    .into_iter()
                    .map(|(key, value)| Ok((key, Self::from_json(value)?)))
                    .collect::<Result<_, LuaRuntimeError>>()?,
            ),
        })
    }

    pub fn to_json(&self) -> Result<serde_json::Value, LuaRuntimeError> {
        Ok(match self {
            Self::Nil => serde_json::Value::Null,
            Self::Boolean(value) => serde_json::Value::Bool(*value),
            Self::Integer(value) => (*value).into(),
            Self::Number(value) => serde_json::Value::Number(
                serde_json::Number::from_f64(*value).ok_or_else(|| {
                    LuaRuntimeError::new(
                        LuaRuntimeErrorKind::InvalidRequest,
                        "non-finite Lua number cannot be JSON",
                    )
                })?,
            ),
            Self::String(value) => serde_json::Value::String(value.clone()),
            Self::Array(values) => serde_json::Value::Array(
                values.iter().map(Self::to_json).collect::<Result<_, _>>()?,
            ),
            Self::Object(values) => serde_json::Value::Object(
                values
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), value.to_json()?)))
                    .collect::<Result<_, LuaRuntimeError>>()?,
            ),
        })
    }
}

pub type HostFuture = Pin<Box<dyn Future<Output = Result<LuaValue, LuaRuntimeError>> + Send>>;

pub trait LuaHostFunction: Send + Sync + 'static {
    fn call(&self, arguments: Vec<LuaValue>, cancel: CancellationToken) -> HostFuture;
}

#[derive(Debug, Default)]
pub struct DriverWakeSlot {
    sender: Option<oneshot::Sender<()>>,
    pending_wake: bool,
    waiting: bool,
    closed: bool,
}

impl DriverWakeSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::new()))
    }

    pub async fn wait(slot: Arc<Mutex<Self>>) -> Result<(), LuaRuntimeError> {
        let receiver = {
            let mut state = lock(&slot);
            if state.closed {
                return Err(LuaRuntimeError::new(
                    LuaRuntimeErrorKind::Closed,
                    "Driver wake slot is closed",
                ));
            }
            if state.waiting {
                return Err(LuaRuntimeError::new(
                    LuaRuntimeErrorKind::AlreadyWaiting,
                    "Driver already has a waiter",
                ));
            }
            if state.pending_wake {
                state.pending_wake = false;
                return Ok(());
            }
            let (sender, receiver) = oneshot::channel();
            state.sender = Some(sender);
            state.waiting = true;
            receiver
        };
        let waiting = WakeWaitGuard(slot);
        let result = receiver.await.map_err(|_| {
            LuaRuntimeError::new(
                LuaRuntimeErrorKind::Closed,
                "Driver wake slot closed during wait",
            )
        });
        drop(waiting);
        result
    }

    pub fn wake(&mut self) -> Result<(), LuaRuntimeError> {
        if self.closed {
            return Err(LuaRuntimeError::new(
                LuaRuntimeErrorKind::Closed,
                "Driver wake slot is closed",
            ));
        }
        if let Some(sender) = self.sender.take() {
            if sender.send(()).is_err() {
                self.pending_wake = true;
            }
        } else {
            self.pending_wake = true;
        }
        Ok(())
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.pending_wake = false;
        self.sender = None;
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

struct WakeWaitGuard(Arc<Mutex<DriverWakeSlot>>);

impl Drop for WakeWaitGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.0);
        state.waiting = false;
        state.sender = None;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaRuntimeConfig {
    pub max_source_bytes: usize,
    pub max_memory_bytes: usize,
    pub instruction_check_interval: u32,
    pub startup_timeout: Option<Duration>,
}

impl Default for LuaRuntimeConfig {
    fn default() -> Self {
        Self {
            max_source_bytes: 1024 * 1024,
            max_memory_bytes: 64 * 1024 * 1024,
            instruction_check_interval: 1000,
            startup_timeout: None,
        }
    }
}

impl LuaRuntimeConfig {
    pub fn validate(&self) -> Result<(), LuaRuntimeError> {
        if self.max_source_bytes == 0
            || self.max_memory_bytes == 0
            || self.instruction_check_interval == 0
            || self
                .startup_timeout
                .is_some_and(|timeout| timeout.is_zero())
        {
            return Err(LuaRuntimeError::new(
                LuaRuntimeErrorKind::InvalidRequest,
                "Lua budgets and optional startup timeout must be positive",
            ));
        }
        Ok(())
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
