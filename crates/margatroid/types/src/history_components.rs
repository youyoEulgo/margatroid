use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub key: String,
    pub source: String,
    pub turn: Option<String>,
    pub kind: String,
    pub content: String,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryError {
    WriteFailed(String),
    KeyConflict(String),
    InvalidRecord(String),
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for HistoryError {}

pub trait AgentHistoryStore: Send + Sync + 'static {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError>;
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError>;
}

#[derive(Clone)]
pub struct AgentHistoryHandle {
    inner: Arc<dyn AgentHistoryStore>,
}
impl std::fmt::Debug for AgentHistoryHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHistoryHandle").finish_non_exhaustive()
    }
}
impl AgentHistoryHandle {
    pub fn new(store: impl AgentHistoryStore) -> Self {
        Self {
            inner: Arc::new(store),
        }
    }
    pub fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        self.inner.append_batch(records)
    }
    pub fn append(&self, record: HistoryRecord) -> Result<(), HistoryError> {
        self.append_batch(vec![record])
    }
    pub fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        self.inner.list()
    }
}
