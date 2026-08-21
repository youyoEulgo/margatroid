use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use core_plugin::Entity;
use margatroid_types::{
    AgentError, AgentErrorKind, Block, BlockAssembly, BlockInner, BlockPath, InnerType, LuaVmId,
    MclDeleteSelection, MclMessage, MclRealtimeSource, Message, RefBlock, RefBlockAssembly,
    RefMerge, ResourceId, TokenUsage, ToolDefinition,
};
use tokio::sync::oneshot;

#[derive(Clone, Debug)]
pub struct AgentCreateReply {
    sender: Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>,
}

impl AgentCreateReply {
    pub fn new(sender: oneshot::Sender<Result<Entity, AgentError>>) -> Self {
        Self {
            sender: Arc::new(Mutex::new(Some(sender))),
        }
    }

    pub(crate) fn send(&self, result: Result<Entity, AgentError>) {
        if let Ok(mut sender) = self.sender.lock() {
            if let Some(sender) = sender.take() {
                let _ = sender.send(result);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum AgentControlKind {
    Stop,
    AbortTurn,
}

#[derive(Clone, Debug)]
pub struct AgentControlReply {
    sender: Arc<Mutex<Option<oneshot::Sender<Result<(), AgentError>>>>>,
}

impl AgentControlReply {
    pub fn new(sender: oneshot::Sender<Result<(), AgentError>>) -> Self {
        Self {
            sender: Arc::new(Mutex::new(Some(sender))),
        }
    }

    pub(crate) fn send(&self, result: Result<(), AgentError>) {
        if let Ok(mut sender) = self.sender.lock() {
            if let Some(sender) = sender.take() {
                let _ = sender.send(result);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentInfo {
    pub image_entity: Entity,
    pub workspace_id: Entity,
    pub model: AgentModelInfo,
    pub project_root: PathBuf,
    pub image_root: PathBuf,
    pub home_root: PathBuf,
    pub image_dependencies: Arc<[ResourceId]>,
    pub image_sources: HashMap<ResourceId, Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentModelInfo {
    pub provider: String,
    pub model: String,
    pub context_window_tokens: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentLifecycleState {
    Creating,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentLuaState {
    pub request_id: Option<String>,
    pub vm_id: Option<LuaVmId>,
}

#[derive(Clone, Debug)]
pub struct AgentCreationState {
    pub request_id: String,
    pub reply: AgentCreateReply,
    pub initialization: AgentInitializationState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentInitializationState {
    pub failed: Option<AgentError>,
    pub complete: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentTurnState {
    pub turn_id: Option<String>,
}

impl AgentTurnState {
    pub fn begin(&mut self, id: String) -> Result<(), AgentError> {
        if id.is_empty() || self.turn_id.is_some() {
            return Err(AgentError::new(
                AgentErrorKind::InvalidRequest,
                "agent turn is already occupied or the turn id is empty",
            ));
        }
        self.turn_id = Some(id);
        Ok(())
    }

    pub fn finish(&mut self, id: &str) -> Result<(), AgentError> {
        if self.turn_id.as_deref() != Some(id) {
            return Err(AgentError::new(
                AgentErrorKind::InvalidRequest,
                "agent turn does not match",
            ));
        }
        self.turn_id = None;
        Ok(())
    }

    pub fn abort(&mut self) -> Option<String> {
        self.turn_id.take()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenUsageState {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_hit_tokens: u64,
    pub cache_hit_rate: f64,
    pub last_input_tokens: u64,
}

impl TokenUsageState {
    pub fn add(&mut self, usage: &TokenUsage) {
        self.total_input_tokens = self.total_input_tokens.saturating_add(usage.input_tokens);
        self.total_output_tokens = self.total_output_tokens.saturating_add(usage.output_tokens);
        self.total_cache_hit_tokens = self
            .total_cache_hit_tokens
            .saturating_add(usage.cache_hit_tokens);
        self.last_input_tokens = usage.input_tokens;
        self.cache_hit_rate = if self.total_input_tokens == 0 {
            0.0
        } else {
            self.total_cache_hit_tokens as f64 / self.total_input_tokens as f64
        };
    }
}

impl From<&TokenUsage> for TokenUsageState {
    fn from(usage: &TokenUsage) -> Self {
        let mut state = Self::default();
        state.add(usage);
        state
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentResourceMap {
    pub resources: BTreeMap<ResourceId, bool>,
    pub aliases: HashMap<String, ResourceId>,
    pub visible: BTreeSet<ResourceId>,
    pub default_visible: BTreeSet<ResourceId>,
    pub visible_source: Option<BlockPath>,
    pub default_visible_source: Option<BlockPath>,
    pub tool_entries: Vec<AgentResourceEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentResourceEntry {
    pub resource_id: ResourceId,
    pub resource_name: String,
    pub tool_id: ResourceId,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl AgentResourceMap {
    pub fn register_tool(&mut self, entry: AgentResourceEntry) -> Result<(), AgentError> {
        if self.tool_entries.iter().any(|existing| {
            existing.resource_id == entry.resource_id
                && existing.resource_name == entry.resource_name
        }) {
            return Ok(());
        }
        if self
            .tool_entries
            .iter()
            .any(|existing| existing.resource_name == entry.resource_name)
        {
            return Err(AgentError::new(
                AgentErrorKind::InvalidRequest,
                "resource name is already registered for this Agent",
            ));
        }
        self.tool_entries.push(entry);
        Ok(())
    }

    pub fn tool_by_name(&self, name: &str) -> Option<&AgentResourceEntry> {
        self.tool_entries
            .iter()
            .find(|entry| entry.resource_name == name)
    }

    pub fn tools_by_resource(&self, resource_id: &ResourceId) -> Vec<&AgentResourceEntry> {
        self.tool_entries
            .iter()
            .filter(|entry| &entry.resource_id == resource_id)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentInferencePending {
    pub id: String,
    pub tool_schema: Vec<ToolDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolPending {
    pub turn_id: String,
    pub tool_call_id: String,
    pub resource_id: ResourceId,
    pub tool_id: ResourceId,
}

#[derive(Clone, Debug)]
pub struct AgentInferenceState {
    pub model: AgentModelInfo,
    pub pending: HashMap<(Entity, String), AgentInferencePending>,
}

#[derive(Clone, Debug, Default)]
pub struct AgentToolState {
    pub pending: HashMap<(Entity, String, String), AgentToolPending>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryMessage {
    pub sequence: i64,
    pub turn_id: String,
    pub message: Message,
    pub tool_schema: Vec<ToolDefinition>,
    pub usage: Option<TokenUsage>,
    pub created_at_ms: i64,
}

pub trait AgentMemoryStore: Send + Sync + 'static {
    fn append_history(
        &self,
        turn_id: &str,
        message: &Message,
        tool_schema: &[ToolDefinition],
        usage: Option<&TokenUsage>,
    ) -> Result<(), AgentMemoryStoreError>;

    fn rewrite_realtime(&self, messages: &[MclMessage]) -> Result<(), AgentMemoryStoreError>;

    fn read_realtime(&self) -> Result<Vec<MclMessage>, AgentMemoryStoreError>;

    fn history_messages(&self) -> Result<Vec<HistoryMessage>, AgentMemoryStoreError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMemoryStoreError {
    pub kind: String,
    pub message: String,
}

impl fmt::Display for AgentMemoryStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for AgentMemoryStoreError {}

#[derive(Clone)]
pub struct AgentMemoryHandle {
    inner: Arc<dyn AgentMemoryStore>,
}

impl AgentMemoryHandle {
    pub fn new(inner: Arc<dyn AgentMemoryStore>) -> Self {
        Self { inner }
    }

    pub fn append_history(
        &self,
        turn_id: &str,
        message: &Message,
        tool_schema: &[ToolDefinition],
        usage: Option<&TokenUsage>,
    ) -> Result<(), AgentMemoryStoreError> {
        self.inner
            .append_history(turn_id, message, tool_schema, usage)
    }

    pub fn rewrite_realtime(&self, messages: &[MclMessage]) -> Result<(), AgentMemoryStoreError> {
        self.inner.rewrite_realtime(messages)
    }

    pub fn read_realtime(&self) -> Result<Vec<MclMessage>, AgentMemoryStoreError> {
        self.inner.read_realtime()
    }

    pub fn history_messages(&self) -> Result<Vec<HistoryMessage>, AgentMemoryStoreError> {
        self.inner.history_messages()
    }
}

impl fmt::Debug for AgentMemoryHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AgentMemoryHandle").finish()
    }
}

#[derive(Clone, Debug, Default)]
pub struct AgentMcl {
    blocks: BlockAssembly,
    ref_blocks: RefBlockAssembly,
    realtime_source: Option<MclRealtimeSource>,
}

impl AgentMcl {
    pub fn blocks(&self) -> &BlockAssembly {
        &self.blocks
    }

    pub fn ref_blocks(&self) -> &RefBlockAssembly {
        &self.ref_blocks
    }

    pub fn select(&self, target: &BlockPath) -> Result<BlockInner, AgentError> {
        if let Some(block) = self.blocks.blocks.get(&target.block_id) {
            return block
                .inners
                .get(&target.inner_id)
                .cloned()
                .ok_or_else(|| AgentError::new(AgentErrorKind::InnerMissing, "inner is missing"));
        }
        self.ref_blocks
            .blocks
            .get(&target.block_id)
            .ok_or_else(|| AgentError::new(AgentErrorKind::BlockMissing, "block is missing"))?
            .merges
            .get(&target.inner_id)
            .ok_or_else(|| AgentError::new(AgentErrorKind::InnerMissing, "merge is missing"))?
            .iter(&self.blocks)
    }

    pub fn merge(&self, sources: &[BlockPath]) -> Result<BlockInner, AgentError> {
        let first = sources.first().ok_or_else(|| {
            AgentError::new(AgentErrorKind::InvalidRequest, "merge source is empty")
        })?;
        let kind = self.select(first)?.inner_type();
        let mut merged = empty_inner(kind);
        for source in sources {
            append_inner(&mut merged, self.select(source)?)?;
        }
        Ok(merged)
    }

    pub fn ref_merge(&self, sources: &[BlockPath]) -> Result<RefMerge, AgentError> {
        let first = sources.first().ok_or_else(|| {
            AgentError::new(AgentErrorKind::InvalidRequest, "reference source is empty")
        })?;
        let kind = self.real_inner(first)?.inner_type();
        for source in &sources[1..] {
            if self.real_inner(source)?.inner_type() != kind {
                return Err(type_mismatch());
            }
        }
        Ok(match kind {
            InnerType::Message => RefMerge::Message(sources.to_vec()),
            InnerType::ToolCall => RefMerge::ToolCall(sources.to_vec()),
            InnerType::ResourceId => RefMerge::ResourceId(sources.to_vec()),
        })
    }

    pub fn create_block(&mut self, block_id: String, block: Block) -> Result<(), AgentError> {
        self.ensure_block_id_available(&block_id)?;
        self.blocks.blocks.insert(block_id, block);
        Ok(())
    }

    pub fn create_ref_block(
        &mut self,
        block_id: String,
        block: RefBlock,
    ) -> Result<(), AgentError> {
        self.ensure_block_id_available(&block_id)?;
        self.ref_blocks.blocks.insert(block_id, block);
        Ok(())
    }

    pub fn insert(&mut self, target: &BlockPath, values: BlockInner) -> Result<(), AgentError> {
        append_inner(self.real_inner_mut(target)?, values)
    }

    pub fn delete(
        &mut self,
        target: &BlockPath,
        selection: MclDeleteSelection,
    ) -> Result<(), AgentError> {
        let slot = self.real_inner_mut(target)?;
        let mut indices = match selection {
            MclDeleteSelection::All => {
                *slot = empty_inner(slot.inner_type());
                return Ok(());
            }
            MclDeleteSelection::First => vec![0],
            MclDeleteSelection::Indices(indices) => indices,
        };
        indices.sort_unstable();
        indices.dedup();
        if indices.iter().any(|index| *index >= slot.len()) {
            return Err(AgentError::new(
                AgentErrorKind::InvalidRequest,
                "delete index is out of range",
            ));
        }
        for index in indices.into_iter().rev() {
            match slot {
                BlockInner::Message(values) => {
                    values.remove(index);
                }
                BlockInner::ToolCall(values) => {
                    values.remove(index);
                }
                BlockInner::ResourceId(values) => {
                    values.remove(index);
                }
            }
        }
        Ok(())
    }

    pub fn cover(&mut self, target: &BlockPath, values: BlockInner) -> Result<(), AgentError> {
        let slot = self.real_inner_mut(target)?;
        if slot.inner_type() != values.inner_type() {
            return Err(type_mismatch());
        }
        *slot = values;
        Ok(())
    }

    pub fn realtime_source(&self) -> Option<&MclRealtimeSource> {
        self.realtime_source.as_ref()
    }

    pub fn set_realtime_source(&mut self, source: MclRealtimeSource) {
        self.realtime_source = Some(source);
    }

    fn ensure_block_id_available(&self, block_id: &str) -> Result<(), AgentError> {
        if self.blocks.blocks.contains_key(block_id)
            || self.ref_blocks.blocks.contains_key(block_id)
        {
            return Err(AgentError::new(
                AgentErrorKind::InvalidRequest,
                "block id already exists",
            ));
        }
        Ok(())
    }

    fn real_inner(&self, target: &BlockPath) -> Result<&BlockInner, AgentError> {
        self.blocks
            .blocks
            .get(&target.block_id)
            .ok_or_else(|| AgentError::new(AgentErrorKind::BlockMissing, "block is missing"))?
            .inners
            .get(&target.inner_id)
            .ok_or_else(|| AgentError::new(AgentErrorKind::InnerMissing, "inner is missing"))
    }

    fn real_inner_mut(&mut self, target: &BlockPath) -> Result<&mut BlockInner, AgentError> {
        self.blocks
            .blocks
            .get_mut(&target.block_id)
            .ok_or_else(|| AgentError::new(AgentErrorKind::BlockMissing, "block is missing"))?
            .inners
            .get_mut(&target.inner_id)
            .ok_or_else(|| AgentError::new(AgentErrorKind::InnerMissing, "inner is missing"))
    }
}

fn empty_inner(kind: InnerType) -> BlockInner {
    match kind {
        InnerType::Message => BlockInner::Message(Vec::new()),
        InnerType::ToolCall => BlockInner::ToolCall(Vec::new()),
        InnerType::ResourceId => BlockInner::ResourceId(Vec::new()),
    }
}

fn append_inner(target: &mut BlockInner, values: BlockInner) -> Result<(), AgentError> {
    match (target, values) {
        (BlockInner::Message(target), BlockInner::Message(values)) => target.extend(values),
        (BlockInner::ToolCall(target), BlockInner::ToolCall(values)) => target.extend(values),
        (BlockInner::ResourceId(target), BlockInner::ResourceId(values)) => target.extend(values),
        _ => return Err(type_mismatch()),
    }
    Ok(())
}

fn type_mismatch() -> AgentError {
    AgentError::new(AgentErrorKind::TypeMismatch, "inner type mismatch")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_reply_sends_at_most_once() {
        let (sender, receiver) = oneshot::channel();
        let reply = AgentCreateReply::new(sender);
        let entity = core_plugin::World::new().spawn();
        reply.send(Ok(entity));
        reply.send(Err(AgentError::new(
            AgentErrorKind::InvalidRequest,
            "second result",
        )));
        assert_eq!(receiver.await.unwrap().unwrap(), entity);
    }
}
