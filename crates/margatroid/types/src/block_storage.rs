use crate::block_types::{BlockInner, BlockPath, InnerType};
use crate::history_components::{
    AgentHistoryHandle, AgentHistoryStore, HistoryError, HistoryRecord,
};
use crate::lua_runtime_components::DriverLuaVmInstance;
use crate::message_types::AgentMessage;
use resource_id_plugin::ResourceID;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredBlockInner {
    Message(Vec<AgentMessage>),
    Tool(Vec<ResourceID>),
    Sandbox(Vec<ResourceID>),
}
impl StoredBlockInner {
    pub fn empty(kind: InnerType) -> Self {
        match kind {
            InnerType::Message => Self::Message(Vec::new()),
            InnerType::Tool => Self::Tool(Vec::new()),
            InnerType::Sandbox => Self::Sandbox(Vec::new()),
        }
    }
    pub fn inner_type(&self) -> InnerType {
        match self {
            Self::Message(_) => InnerType::Message,
            Self::Tool(_) => InnerType::Tool,
            Self::Sandbox(_) => InnerType::Sandbox,
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Message(values) => values.len(),
            Self::Tool(values) | Self::Sandbox(values) => values.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn slice(&self, start: usize, end: usize) -> Result<Self, BlockStorageError> {
        match self {
            Self::Message(values) => values.get(start..end).map(|v| Self::Message(v.to_vec())),
            Self::Tool(values) => values.get(start..end).map(|v| Self::Tool(v.to_vec())),
            Self::Sandbox(values) => values.get(start..end).map(|v| Self::Sandbox(v.to_vec())),
        }
        .ok_or(BlockStorageError::InvalidRange)
    }
    pub fn append(&mut self, other: Self) -> Result<(), BlockStorageError> {
        match (self, other) {
            (Self::Message(a), Self::Message(b)) => a.extend(b),
            (Self::Tool(a), Self::Tool(b)) | (Self::Sandbox(a), Self::Sandbox(b)) => a.extend(b),
            _ => return Err(BlockStorageError::TypeMismatch),
        }
        Ok(())
    }
    pub fn replace(
        &mut self,
        start: usize,
        end: usize,
        other: Self,
    ) -> Result<(), BlockStorageError> {
        if start > end || end > self.len() {
            return Err(BlockStorageError::InvalidRange);
        }
        match (self, other) {
            (Self::Message(a), Self::Message(b)) => {
                a.splice(start..end, b);
            }
            (Self::Tool(a), Self::Tool(b)) | (Self::Sandbox(a), Self::Sandbox(b)) => {
                a.splice(start..end, b);
            }
            _ => return Err(BlockStorageError::TypeMismatch),
        }
        Ok(())
    }
}
impl From<BlockInner> for StoredBlockInner {
    fn from(value: BlockInner) -> Self {
        match value {
            BlockInner::Message(values) => Self::Message(values),
            BlockInner::Tool(values) => {
                Self::Tool(values.into_iter().map(|v| v.resource_id).collect())
            }
            BlockInner::Sandbox(values) => {
                Self::Sandbox(values.into_iter().map(|v| v.resource_id).collect())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockNamespace {
    Persistent { agent: ResourceID },
    Temporary { instance: DriverLuaVmInstance },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockStorageContext {
    pub agent: ResourceID,
    pub instance: DriverLuaVmInstance,
}
impl BlockStorageContext {
    pub fn namespace(&self, temporary: bool) -> BlockNamespace {
        if temporary {
            BlockNamespace::Temporary {
                instance: self.instance.clone(),
            }
        } else {
            BlockNamespace::Persistent {
                agent: self.agent.clone(),
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSchema {
    pub name: String,
    pub fields: Vec<(String, InnerType)>,
    pub temporary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRange {
    All,
    Empty,
    Single(i64),
    Range(Option<i64>, Option<i64>),
}
impl BlockRange {
    pub fn map(self, length: usize, write: bool) -> Result<(usize, usize), BlockStorageError> {
        let length = i64::try_from(length).map_err(|_| BlockStorageError::InvalidRange)?;
        let offset = length
            .checked_add(1)
            .ok_or(BlockStorageError::InvalidRange)?;
        let endpoint = |value: i64| {
            if value < 0 {
                offset.checked_add(value)
            } else {
                Some(value)
            }
        };
        let (start, end) = match self {
            Self::All => (0, length),
            Self::Empty => (0, 0),
            Self::Single(0) if length == 0 && !write => (0, 0),
            Self::Single(value) => {
                let end = endpoint(value).ok_or(BlockStorageError::InvalidRange)?;
                if write {
                    (end, end)
                } else if value < 0 {
                    (
                        end.checked_sub(1).ok_or(BlockStorageError::InvalidRange)?,
                        end,
                    )
                } else {
                    (
                        end,
                        end.checked_add(1).ok_or(BlockStorageError::InvalidRange)?,
                    )
                }
            }
            Self::Range(None, None) => return Err(BlockStorageError::InvalidRange),
            Self::Range(start, end) => {
                if matches!((start, end), (Some(a), Some(b)) if (a < 0) != (b < 0)) {
                    return Err(BlockStorageError::InvalidRange);
                }
                (
                    match start {
                        Some(v) => endpoint(v).ok_or(BlockStorageError::InvalidRange)?,
                        None => 0,
                    },
                    match end {
                        Some(v) => endpoint(v).ok_or(BlockStorageError::InvalidRange)?,
                        None => length,
                    },
                )
            }
        };
        if start < 0 || start > end || end > length {
            return Err(BlockStorageError::InvalidRange);
        }
        Ok((start as usize, end as usize))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSelector {
    pub path: BlockPath,
    pub range: BlockRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockSource {
    Values(StoredBlockInner),
    Selector(BlockSelector),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockExpression {
    Sources(Vec<BlockSource>),
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockStorageOperation {
    Create(BlockSchema),
    Get {
        expressions: Vec<BlockExpression>,
    },
    Inject {
        source: BlockExpression,
        target: BlockSelector,
    },
}

#[derive(Clone, Debug)]
pub struct BlockStorageRequest {
    pub context: BlockStorageContext,
    pub operation: BlockStorageOperation,
    pub cancel: CancellationToken,
}
impl BlockStorageRequest {
    pub fn check_cancelled(&self) -> Result<(), BlockStorageError> {
        if self.cancel.is_cancelled() {
            Err(BlockStorageError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockOpened {
    pub block_id: String,
    pub created: bool,
    pub temporary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredBlockResult {
    BlockOpened(BlockOpened),
    Arrays(Vec<StoredBlockInner>),
    Unit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockStorageError {
    SchemaMismatch(String),
    LifecycleMismatch(String),
    BlockMissing(String),
    FieldMissing(BlockPath),
    TypeMismatch,
    InvalidRange,
    InvalidRequest(String),
    Cancelled,
    CommitUnknown(String),
    CleanupFailed(String),
    Io(String),
}
impl std::fmt::Display for BlockStorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BlockStorageError {}

/// A blocking transactional backend. Cancellation means confirmed not committed;
/// a confirmed commit must return its success even if cancellation raced it.
pub trait AgentBlockStorage: Send + Sync + 'static {
    fn execute(&self, request: BlockStorageRequest)
        -> Result<StoredBlockResult, BlockStorageError>;
}
pub trait AgentDatabaseStore: AgentHistoryStore + AgentBlockStorage {}
impl<T: AgentHistoryStore + AgentBlockStorage> AgentDatabaseStore for T {}

#[derive(Clone)]
pub struct AgentDatabaseHandle {
    store: Arc<dyn AgentDatabaseStore>,
}
impl std::fmt::Debug for AgentDatabaseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDatabaseHandle")
            .finish_non_exhaustive()
    }
}
impl AgentDatabaseHandle {
    pub fn new(store: impl AgentDatabaseStore) -> Self {
        Self {
            store: Arc::new(store),
        }
    }
    pub fn history(&self) -> AgentHistoryHandle {
        AgentHistoryHandle::new(HistoryView(self.store.clone()))
    }
    pub fn block_storage(&self) -> AgentBlockStorageHandle {
        AgentBlockStorageHandle {
            store: self.store.clone(),
        }
    }
}
struct HistoryView(Arc<dyn AgentDatabaseStore>);
impl AgentHistoryStore for HistoryView {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        self.0.append_batch(records)
    }
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        self.0.list()
    }
}

#[derive(Clone)]
pub struct AgentBlockStorageHandle {
    store: Arc<dyn AgentDatabaseStore>,
}
impl std::fmt::Debug for AgentBlockStorageHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBlockStorageHandle")
            .finish_non_exhaustive()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_preserve_read_write_and_empty_array_semantics() {
        assert_eq!(BlockRange::Single(-1).map(4, false), Ok((3, 4)));
        assert_eq!(BlockRange::Single(-1).map(4, true), Ok((4, 4)));
        assert_eq!(BlockRange::Single(0).map(0, false), Ok((0, 0)));
        assert_eq!(
            BlockRange::Range(Some(-3), Some(-1)).map(4, false),
            Ok((2, 4))
        );
        assert_eq!(BlockRange::Empty.map(4, true), Ok((0, 0)));
        for range in [
            BlockRange::Range(None, None),
            BlockRange::Range(Some(-1), Some(1)),
            BlockRange::Single(i64::MAX),
            BlockRange::Single(i64::MIN),
            BlockRange::Range(Some(3), Some(2)),
        ] {
            assert_eq!(range.map(4, false), Err(BlockStorageError::InvalidRange));
        }
        assert_eq!(
            BlockRange::All.map(usize::MAX, false),
            Err(BlockStorageError::InvalidRange)
        );
    }

    #[test]
    fn stored_empty_types_and_failed_replacement_are_preserved() {
        let mut value = StoredBlockInner::Tool(Vec::new());
        assert_eq!(
            value.append(StoredBlockInner::Sandbox(Vec::new())),
            Err(BlockStorageError::TypeMismatch)
        );
        assert_eq!(
            value.replace(0, 0, StoredBlockInner::Message(Vec::new())),
            Err(BlockStorageError::TypeMismatch)
        );
        assert_eq!(value.inner_type(), InnerType::Tool);
        assert_eq!(value.slice(0, 0).unwrap(), value);
        let encoded = serde_json::to_string(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<StoredBlockInner>(&encoded).unwrap(),
            value
        );
    }
}

impl AgentBlockStorageHandle {
    /// The caller owns one Other activity until this actual worker join completes.
    /// Dropping this future never proves rollback or completion of that activity.
    pub async fn execute(
        &self,
        request: BlockStorageRequest,
    ) -> Result<StoredBlockResult, BlockStorageError> {
        request.check_cancelled()?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            request.check_cancelled()?;
            AgentBlockStorage::execute(&*store, request)
        })
        .await
        .map_err(|error| {
            BlockStorageError::CommitUnknown(format!("storage worker join failed: {error}"))
        })?
    }
}
