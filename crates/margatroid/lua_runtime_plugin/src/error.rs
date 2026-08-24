use std::fmt;

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
