use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use margatroid_types::{ResourceId, ToolCall};
use sha2::{Digest, Sha256};

pub use agent_plugin::AgentMcl;
pub use margatroid_types::{
    Block, BlockAssembly, BlockInner, BlockPath, InnerType, MclMessage, MclRealtimeSource,
    RefBlock, RefBlockAssembly, RefMerge,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MclHash(String);
impl MclHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    fn digest(source: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(source.as_bytes());
        Self(format!("{:x}", digest.finalize()))
    }
}
impl std::fmt::Display for MclHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclProgramKind {
    Base,
    Workflow,
    Module,
}

#[derive(Clone, Debug)]
pub struct MclSource {
    resource_id: ResourceId,
    source: Arc<str>,
    origin: Arc<PathBuf>,
}
impl MclSource {
    pub fn new(
        resource_id: ResourceId,
        source: impl Into<Arc<str>>,
        origin: impl Into<Arc<PathBuf>>,
    ) -> Self {
        Self {
            resource_id,
            source: source.into(),
            origin: origin.into(),
        }
    }
    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn origin(&self) -> &Path {
        self.origin.as_path()
    }
}

#[derive(Clone, Debug)]
pub struct MclProgram {
    resource_id: ResourceId,
    source: Arc<str>,
    origin: Arc<PathBuf>,
    kind: MclProgramKind,
    source_hash: MclHash,
    plan_hash: MclHash,
}
impl MclProgram {
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn origin(&self) -> &Path {
        self.origin.as_path()
    }
    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }
    pub fn kind(&self) -> MclProgramKind {
        self.kind
    }
    pub fn source_hash(&self) -> &MclHash {
        &self.source_hash
    }
    pub fn plan_hash(&self) -> &MclHash {
        &self.plan_hash
    }
}

#[derive(Clone, Debug)]
pub struct MclCompileRequest {
    pub root: MclSource,
    pub dependencies: BTreeMap<ResourceId, MclSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceImportReceipt {
    pub resource_id: ResourceId,
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
#[derive(Clone, Debug)]
pub enum MclPredicate {
    IdEquals(String),
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
    Start,
    CatchInference { ref_block_id: String },
    Inference { ref_block_id: String },
    ToolCall { calls: Vec<ToolCall> },
    Finish,
    HistoryAppend { message: MclMessage },
    RealtimeSource { ref_block_id: String },
    VisibilitySource { source: BlockPath },
    DefaultVisibilitySource { source: BlockPath },
    RealtimeLoad,
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
        resource_id: ResourceId,
        alias: String,
    },
    Inject {
        target: BlockPath,
        value: MclBinding,
    },
    InjectMany {
        target: BlockPath,
        values: Vec<MclBinding>,
    },
    CoverValue {
        target: BlockPath,
        value: MclBinding,
    },
    CoverInner {
        source: BlockPath,
        target: BlockPath,
    },
    Select {
        source: BlockPath,
    },
    DeleteAll {
        target: BlockPath,
    },
    DeleteFirst {
        target: BlockPath,
    },
    DeleteWhere {
        target: BlockPath,
        predicate: MclPredicate,
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
#[derive(Clone, Debug)]
pub enum MclEffect {
    Start,
    CatchInference {
        messages: Vec<MclMessage>,
    },
    Inference {
        messages: Vec<MclMessage>,
        visible_resources: Vec<ResourceId>,
    },
    ToolCall {
        calls: Vec<ToolCall>,
    },
    Finish,
    HistoryAppend {
        message: MclMessage,
    },
    RealtimeSource {
        source: MclRealtimeSource,
        values: Vec<MclMessage>,
    },
    RealtimeLoad,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclPendingEffectKind {
    Start { vm_id: margatroid_types::LuaVmId },
    CatchInference,
    RealtimeLoad,
}

pub fn compile_mcl(request: MclCompileRequest) -> Result<Arc<MclProgram>, crate::MclError> {
    let source = request.root.source().to_owned();
    let hash = MclHash::digest(&source);
    Ok(Arc::new(MclProgram {
        resource_id: request.root.resource_id().clone(),
        source: Arc::from(source),
        origin: Arc::new(request.root.origin().to_path_buf()),
        kind: MclProgramKind::Base,
        source_hash: hash.clone(),
        plan_hash: hash,
    }))
}

pub fn load_mcl_program_from_path(
    _roots: &[PathBuf],
    resource_id: &ResourceId,
    path: &Path,
    expected: MclProgramKind,
) -> Result<Arc<MclProgram>, crate::MclError> {
    let source = String::from_utf8(fs::read(path).map_err(|_| crate::MclError::SourceReadFailed)?)
        .map_err(|_| crate::MclError::SourceInvalidUtf8)?;
    let program = compile_mcl(MclCompileRequest {
        root: MclSource::new(
            resource_id.clone(),
            Arc::<str>::from(source),
            Arc::new(path.to_path_buf()),
        ),
        dependencies: BTreeMap::new(),
    })?;
    if program.kind() != expected {
        return Err(crate::MclError::InvalidProgramKind);
    }
    Ok(program)
}
