//! Shared Lua host object. Domain integrations supply plain callback functions.
use mlua::{
    AnyUserData, Function, Lua, LuaSerdeExt, MultiValue, Table, UserData, UserDataFields,
    UserDataMethods,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};
pub mod data;
pub mod diagnostic;
pub mod process;
pub use diagnostic::{LuaFailureClass, LuaFailureDiagnostic};

/// Run named operations concurrently, dropping the remaining futures after one completes.
pub fn install_wait_any(lua: &Lua) -> mlua::Result<Function> {
    lua.create_async_function(|_, operations: Table| async move {
        use futures_util::{StreamExt, stream::FuturesUnordered};
        let mut pending = FuturesUnordered::new();
        for pair in operations.pairs::<String, Function>() {
            let (name, function) = pair?;
            pending.push(async move {
                function
                    .call_async::<mlua::Value>(())
                    .await
                    .map(|value| (name, value))
            });
        }
        pending.next().await.ok_or_else(|| {
            mlua::Error::external(SharedHostError::new(
                SharedHostErrorKind::InvalidArguments,
                "wait_any requires an operation",
            ))
        })?
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    Driver,
    Tool,
}
impl HostKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Driver => "driver",
            Self::Tool => "tool",
        }
    }
}
#[derive(Clone, Debug)]
pub struct HostDefinition {
    pub kind: HostKind,
    pub context: Value,
    pub arguments: Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedHostErrorKind {
    InvalidArguments,
    InvalidRequest,
    InvalidExecutionContext,
    Unavailable,
    ExecutionFailed,
    RunnerStartFailed,
    CleanupFailed,
    Closed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedHostError {
    pub kind: SharedHostErrorKind,
    pub description: String,
}
impl SharedHostError {
    pub fn new(kind: SharedHostErrorKind, description: impl Into<String>) -> Self {
        Self {
            kind,
            description: description.into(),
        }
    }
}
impl std::fmt::Display for SharedHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.description)
    }
}
impl std::error::Error for SharedHostError {}
type Callbacks = Arc<Mutex<Option<std::collections::BTreeMap<String, Function>>>>;
pub struct Margatroid {
    definition: HostDefinition,
    callbacks: Callbacks,
    namespaces: Vec<AnyUserData>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NamespaceKind {
    Files,
    Data,
    Text,
    Process,
    Driver,
    Tools,
    Task,
}
impl NamespaceKind {
    const ALL: [Self; 7] = [
        Self::Files,
        Self::Data,
        Self::Text,
        Self::Process,
        Self::Driver,
        Self::Tools,
        Self::Task,
    ];
    fn name(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Data => "data",
            Self::Text => "text",
            Self::Process => "process",
            Self::Driver => "driver",
            Self::Tools => "tools",
            Self::Task => "task",
        }
    }
    fn allowed(self, owner: HostKind) -> bool {
        match self {
            Self::Driver => owner == HostKind::Driver,
            Self::Tools | Self::Task => owner == HostKind::Tool,
            _ => true,
        }
    }
}
struct HostNamespace {
    kind: NamespaceKind,
    owner: HostKind,
    callbacks: Callbacks,
}
const ROOT_METHODS: &[&str] = &["log", "wait_any"];
const NAMESPACE_METHODS: &[(NamespaceKind, &str, &str, bool)] = &[
    (NamespaceKind::Files, "read", "files_read", true),
    (NamespaceKind::Files, "write", "files_write", true),
    (NamespaceKind::Files, "list", "files_list", true),
    (NamespaceKind::Files, "info", "files_info", true),
    (
        NamespaceKind::Files,
        "create_directory",
        "files_create_directory",
        true,
    ),
    (NamespaceKind::Files, "matches", "files_matches", false),
    (NamespaceKind::Data, "encode", "data_encode", false),
    (NamespaceKind::Data, "decode", "data_decode", false),
    (NamespaceKind::Text, "search", "text_search", false),
    (NamespaceKind::Process, "start", "process_start", true),
    (NamespaceKind::Driver, "mcl", "driver_mcl", true),
    (NamespaceKind::Driver, "wait", "driver_wait", true),
    (NamespaceKind::Tools, "register", "tools_register", true),
    (
        NamespaceKind::Task,
        "return_result",
        "task_return_result",
        true,
    ),
    (NamespaceKind::Task, "finish", "task_finish", true),
    (
        NamespaceKind::Task,
        "append_output",
        "task_append_output",
        true,
    ),
    (
        NamespaceKind::Task,
        "replace_output",
        "task_replace_output",
        true,
    ),
    (NamespaceKind::Task, "read_output", "task_read_output", true),
    (NamespaceKind::Task, "send", "task_send", true),
    (NamespaceKind::Task, "receive", "task_receive", true),
];
fn unavailable(message: &str) -> mlua::Error {
    mlua::Error::external(SharedHostError::new(
        SharedHostErrorKind::Unavailable,
        message,
    ))
}
fn invalid_context() -> mlua::Error {
    mlua::Error::external(SharedHostError::new(
        SharedHostErrorKind::InvalidExecutionContext,
        "method unavailable in this execution context",
    ))
}
fn callback(callbacks: &Callbacks, name: &str) -> mlua::Result<Function> {
    let state = callbacks.lock().unwrap_or_else(|e| e.into_inner());
    let table = state
        .as_ref()
        .ok_or_else(|| unavailable("margatroid methods unavailable during configure"))?;
    table
        .get(name)
        .cloned()
        .ok_or_else(|| unavailable(&format!("margatroid callback {name} unavailable")))
}
impl HostNamespace {
    fn callback(&self, kind: NamespaceKind, key: &str) -> mlua::Result<Function> {
        if self.kind != kind || !kind.allowed(self.owner) {
            return Err(invalid_context());
        }
        callback(&self.callbacks, key)
    }
}
impl Margatroid {
    pub fn install(lua: &Lua, mut definition: HostDefinition) -> mlua::Result<AnyUserData> {
        let context = definition.context.as_object_mut().ok_or_else(|| {
            mlua::Error::external(SharedHostError::new(
                SharedHostErrorKind::InvalidArguments,
                "host context must be an object",
            ))
        })?;
        context.insert("kind".into(), definition.kind.as_str().into());
        context.insert("arguments".into(), definition.arguments.clone());
        let callbacks = Arc::new(Mutex::new(None));
        let namespaces = NamespaceKind::ALL
            .into_iter()
            .map(|kind| {
                lua.create_userdata(HostNamespace {
                    kind,
                    owner: definition.kind,
                    callbacks: callbacks.clone(),
                })
            })
            .collect::<mlua::Result<Vec<_>>>()?;
        let object = lua.create_userdata(Self {
            definition,
            callbacks,
            namespaces,
        })?;
        lua.globals().set("margatroid", object.clone())?;
        Ok(object)
    }
    /// Freeze the callback selection at the configure/execute boundary.
    pub fn enable(object: &AnyUserData, supplied: Table) -> mlua::Result<()> {
        let host = object.borrow::<Self>()?;
        let mut state = host.callbacks.lock().unwrap_or_else(|e| e.into_inner());
        if state.is_some() {
            return Err(unavailable("margatroid methods already enabled"));
        }
        let mut frozen = std::collections::BTreeMap::new();
        for &key in ROOT_METHODS {
            if let Some(function) = supplied.get::<Option<Function>>(key)? {
                frozen.insert(key.to_owned(), function);
            }
        }
        for &(kind, _, key, _) in NAMESPACE_METHODS {
            if kind.allowed(host.definition.kind)
                && let Some(function) = supplied.get::<Option<Function>>(key)?
            {
                frozen.insert(key.to_owned(), function);
            }
        }
        *state = Some(frozen);
        Ok(())
    }
}
impl UserData for Margatroid {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("context", |lua, host| {
            data::to_lua(lua, host.definition.context.clone())
        });
        for (index, kind) in NamespaceKind::ALL.into_iter().enumerate() {
            fields.add_field_method_get(kind.name(), move |_, host| {
                Ok(host.namespaces[index].clone())
            });
        }
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        for &name in ROOT_METHODS {
            methods.add_async_function(
                name,
                move |_, (object, args): (AnyUserData, MultiValue)| {
                    let function = object
                        .borrow::<Self>()
                        .and_then(|host| callback(&host.callbacks, name));
                    async move { function?.call_async::<MultiValue>(args).await }
                },
            );
        }
    }
}
impl UserData for HostNamespace {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("null", |lua, host| {
            if host.kind != NamespaceKind::Data {
                return Err(invalid_context());
            }
            Ok(lua.null())
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        for &(kind, name, key, asynchronous) in NAMESPACE_METHODS {
            if asynchronous {
                methods.add_async_function(
                    name,
                    move |_, (object, args): (AnyUserData, MultiValue)| {
                        let function = object
                            .borrow::<Self>()
                            .and_then(|host| host.callback(kind, key));
                        async move { function?.call_async::<MultiValue>(args).await }
                    },
                );
            } else {
                methods.add_function(name, move |_, (object, args): (AnyUserData, MultiValue)| {
                    let function = object.borrow::<Self>()?.callback(kind, key)?;
                    function.call::<MultiValue>(args)
                });
            }
        }
    }
}
