use std::sync::Arc;

use margatroid_types::{ResourceRef, ToolCall};

pub use agent_plugin::AgentMcl;
pub use margatroid_types::{
    Block, BlockAssembly, BlockInner, BlockPath, InnerType, MclMessage, RefBlock, RefBlockAssembly,
    RefMerge,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceImportReceipt {
    pub resource_id: ResourceRef,
    pub alias: String,
    pub available: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MclCommandId(String);
impl MclCommandId {
    pub fn new(value: impl Into<String>) -> Result<Self, crate::MclError> {
        let value = value.into();
        (!value.is_empty())
            .then_some(Self(value))
            .ok_or(crate::MclError::InvalidCommand)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct MclCommandReply {
    sender: Arc<
        std::sync::Mutex<
            Option<tokio::sync::oneshot::Sender<Result<MclCommandValue, crate::MclError>>>,
        >,
    >,
}
impl MclCommandReply {
    pub fn new(
        sender: tokio::sync::oneshot::Sender<Result<MclCommandValue, crate::MclError>>,
    ) -> Self {
        Self {
            sender: Arc::new(std::sync::Mutex::new(Some(sender))),
        }
    }
    pub fn send(&self, result: Result<MclCommandValue, crate::MclError>) {
        if let Ok(mut sender) = self.sender.lock() {
            if let Some(sender) = sender.take() {
                let _ = sender.send(result);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct MclBinding(pub serde_json::Value);
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclSelector {
    All(BlockPath),
    Raw {
        path: BlockPath,
        range: MclRange,
        position: MclSelectorPosition,
    },
    Standard {
        path: BlockPath,
        start: usize,
        end: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclRange {
    Empty,
    Single(i64),
    Range(MclEndpoint, MclEndpoint),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclSelectorPosition {
    Get,
    Inject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclEndpoint {
    Value(i64),
    Open,
}

#[derive(Clone, Debug)]
pub enum MclInjectSource {
    Selector(MclSelector),
    Bindings(Vec<MclBinding>),
}
#[derive(Clone, Debug)]
pub enum BlockFieldDeclaration {
    Empty {
        inner_id: String,
        inner_type: InnerType,
    },
    Merge {
        inner_id: String,
        sources: Vec<BlockPath>,
    },
}
#[derive(Clone, Debug)]
pub struct RefMergeDeclaration {
    pub merge_id: String,
    pub sources: Vec<BlockPath>,
}
#[derive(Clone, Debug)]
pub enum MclEffectCommand {
    Wake,
    Ready,
    StartTurn,
    Inference {
        ref_block_id: String,
    },
    ToolCall {
        calls: Vec<ToolCall>,
    },
    Finish,
    HistoryAppend {
        message: MclMessage,
    },
    HistoryRecord {
        kind: String,
        content: String,
        payload: String,
    },
    SandboxUse {
        aliases: Vec<String>,
    },
    VisibilitySource {
        source: BlockPath,
    },
    DefaultVisibilitySource {
        source: BlockPath,
    },
}
#[derive(Clone, Debug)]
pub enum MclOperation {
    CreateBlock {
        block_id: String,
        fields: Vec<BlockFieldDeclaration>,
    },
    CreateRefBlock {
        block_id: String,
        merges: Vec<RefMergeDeclaration>,
    },
    Merge {
        sources: Vec<BlockPath>,
    },
    RefMerge {
        sources: Vec<BlockPath>,
    },
    Import {
        resource_id: ResourceRef,
        alias: String,
    },
    Get {
        selector: MclSelector,
    },
    Inject {
        source: MclInjectSource,
        target: MclSelector,
    },
    BindState {
        block_id: String,
        state_name: String,
    },
    LoadState {
        state_name: String,
        block_id: String,
    },
    Expose {
        domain: String,
        mappings: Vec<(String, String)>,
    },
    Emit {
        effect: MclEffectCommand,
    },
}
#[derive(Clone, Debug)]
pub enum MclDomainValue {
    Unit,
    Inner(BlockInner),
    Paths(Vec<BlockPath>),
    Message(MclMessage),
    ResourceImport(ResourceImportReceipt),
    Text(String),
}
pub type MclCommandValue = MclDomainValue;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclPendingEffectKind {
    Inference,
    ToolCall { expected: usize },
}
