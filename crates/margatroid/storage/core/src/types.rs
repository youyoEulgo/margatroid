use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::{atomic::AtomicBool, Arc, Mutex};

#[derive(Clone)]
pub struct StorageDatabase {
    pub(crate) shared: Arc<DatabaseState>,
}
pub(crate) struct DatabaseState {
    pub(crate) connection: Mutex<Connection>,
    pub(crate) retired: AtomicBool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StorageOperation {
    CreateBlock {
        name: String,
        fields: Vec<String>,
        instance: Option<String>,
    },
    ReadMessages {
        selectors: Vec<MessageSelector>,
    },
    ReadMessageSources {
        sources: Vec<MessageSource>,
    },
    ReplaceMessages {
        sources: Vec<MessageSource>,
        target: MessageSelector,
    },
    ReadConfig,
    SetConfig {
        path: String,
        value: toml::Value,
    },
    DeleteConfig {
        path: String,
    },
    AppendHistory {
        records: Vec<HistoryRecord>,
    },
    ReadHistory,
    ReadHistoryEntries,
    ReadAgentMessages,
    ReadMclRecords,
    DropTemporaryBlocks {
        instance: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageSelector {
    pub block: String,
    pub field: String,
    pub instance: Option<String>,
    pub selection: MessageSelection,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageSelection {
    All,
    Index(i64),
    Range(i64, i64),
}
#[derive(Clone, Debug, PartialEq)]
pub enum MessageSource {
    Stored(MessageSelector),
    Literal(Vec<serde_json::Value>),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub source: Option<String>,
    pub turn: Option<String>,
    pub kind: String,
    pub content: String,
    pub payload: serde_json::Value,
    pub created_at_ms: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredHistoryRecord {
    pub sequence: u64,
    pub record: HistoryRecord,
}
#[derive(Clone, Debug, PartialEq)]
pub enum StorageValue {
    Unit,
    Messages(Vec<serde_json::Value>),
    Config(toml::Table),
    History(Vec<HistoryRecord>),
    HistoryEntries(Vec<StoredHistoryRecord>),
}
