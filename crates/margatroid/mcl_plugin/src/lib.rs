mod driver;
mod runtime;
mod syntax;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::path::{Component as PathComponent, Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource};
use margatroid_types::{Message, ResourceId, TokenUsage, ToolCall};
use sha2::{Digest, Sha256};

pub use driver::spawn_base_driver;
pub use runtime::WorldMclExt;
pub use syntax::{
    MclBlockDefinition, MclBlockLifetime, MclBlockType, MclHandler, MclPredicate,
    MclRequestDefinition, MclStatement, MclViewDefinition,
};

const MAX_ERROR_BYTES: usize = 512;
const MAX_SOURCE_BYTES: u64 = 1024 * 1024;
const COMPILER_VERSION: &str = "mcl-v0.1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclProgramKind {
    Base,
    Workflow,
    Module,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MclHash(String);

impl MclHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn digest(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> Self {
        let mut digest = Sha256::new();
        for part in parts {
            let bytes = part.as_ref();
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
        let bytes = digest.finalize();
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use fmt::Write;
            write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self(output)
    }
}

impl fmt::Display for MclHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct MclSource {
    resource_id: ResourceId,
    source: Arc<str>,
    origin: Arc<PathBuf>,
}

/// The source of an executable MCL Driver.  Unlike the legacy `MclProgram`,
/// this value is not a compiled handler graph: the Lua source is the program
/// and its control flow is executed by the per-agent Driver.
#[derive(Clone, Debug)]
pub struct MclDriverSource {
    resource_id: ResourceId,
    source: Arc<str>,
    origin: Arc<PathBuf>,
    source_hash: MclHash,
}

impl MclDriverSource {
    pub fn new(
        resource_id: ResourceId,
        source: impl Into<Arc<str>>,
        origin: impl Into<Arc<PathBuf>>,
    ) -> Self {
        let source = source.into();
        let source_hash = MclHash::digest([source.as_bytes()]);
        Self {
            resource_id,
            source,
            origin: origin.into(),
            source_hash,
        }
    }

    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn origin(&self) -> &Path {
        &self.origin
    }

    pub fn source_hash(&self) -> &MclHash {
        &self.source_hash
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MclCommandValue {
    Unit,
    Json(serde_json::Value),
}

#[derive(Clone, Debug)]
pub struct MclCommandReceived {
    pub id: String,
    pub agent: Entity,
    pub command: String,
    pub binding: Option<serde_json::Value>,
    pub reply: Sender<Result<MclCommandValue, MclError>>,
}

impl Event for MclCommandReceived {}

#[derive(Clone, Debug)]
pub struct MclRuntimeMessage {
    pub id: String,
    pub agent: Entity,
    pub message: MclMessage,
}

impl Event for MclRuntimeMessage {}

#[derive(Clone, Debug)]
pub struct MclBlockingInferenceRequest {
    pub id: String,
    pub agent: Entity,
    pub request: String,
    pub reply: Sender<Result<MclCommandValue, MclError>>,
}

impl Event for MclBlockingInferenceRequest {}

#[derive(Clone, Debug)]
pub struct MclHistoryAppendRequested {
    pub id: String,
    pub agent: Entity,
    pub message: MclMessage,
}

impl Event for MclHistoryAppendRequested {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MclMessage {
    pub message: Message,
    pub usage: Option<TokenUsage>,
}

impl MclMessage {
    pub fn new(message: Message, usage: Option<TokenUsage>) -> Self {
        Self { message, usage }
    }

    pub fn message(&self) -> &Message {
        &self.message
    }

    pub fn usage(&self) -> Option<&TokenUsage> {
        self.usage.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MclDriverReady {
    pub agent: Entity,
}

impl Event for MclDriverReady {}

#[derive(Clone, Debug)]
pub struct StartMclDriver {
    pub agent: Entity,
}

impl Event for StartMclDriver {}

#[derive(Clone, Debug, PartialEq)]
pub struct MclDriverFailed {
    pub agent: Entity,
    pub error: String,
}

impl Event for MclDriverFailed {}

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
        &self.origin
    }
}

#[derive(Clone, Debug)]
pub struct MclCompileRequest {
    pub root: MclSource,
    pub dependencies: BTreeMap<ResourceId, MclSource>,
}

#[derive(Clone, Debug)]
pub struct MclProgramDependency {
    pub resource_id: ResourceId,
    pub source_hash: MclHash,
    pub origin: Arc<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct MclProgram {
    resource_id: ResourceId,
    source: Arc<str>,
    origin: Arc<PathBuf>,
    kind: MclProgramKind,
    name: String,
    source_hash: MclHash,
    plan_hash: MclHash,
    imports: Vec<MclProgramDependency>,
    blocks: Vec<MclBlockDefinition>,
    views: Vec<MclViewDefinition>,
    requests: Vec<MclRequestDefinition>,
    handlers: Vec<MclHandler>,
}

impl MclProgram {
    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn origin(&self) -> &Path {
        &self.origin
    }

    pub fn driver_source(&self) -> MclDriverSource {
        MclDriverSource::new(
            self.resource_id.clone(),
            Arc::clone(&self.source),
            Arc::clone(&self.origin),
        )
    }

    pub fn kind(&self) -> MclProgramKind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn source_hash(&self) -> &MclHash {
        &self.source_hash
    }

    pub fn plan_hash(&self) -> &MclHash {
        &self.plan_hash
    }

    pub fn imports(&self) -> &[MclProgramDependency] {
        &self.imports
    }

    pub fn blocks(&self) -> &[MclBlockDefinition] {
        &self.blocks
    }

    pub fn views(&self) -> &[MclViewDefinition] {
        &self.views
    }

    pub fn requests(&self) -> &[MclRequestDefinition] {
        &self.requests
    }

    pub fn handlers(&self) -> &[MclHandler] {
        &self.handlers
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkflowInstanceId(String);

impl WorkflowInstanceId {
    pub fn new(value: impl Into<String>) -> Result<Self, MclError> {
        let value = value.into();
        if value.is_empty() || value.contains("::") || value.chars().any(char::is_control) {
            return Err(MclError::new(
                MclErrorKind::InvalidEvent,
                "Workflow instance ID is invalid",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkflowInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MclCapabilityOwner {
    Base,
    Workflow(WorkflowInstanceId),
    External(String),
}

#[derive(Clone, Debug, Default)]
pub struct MclCapabilityStore {
    default: BTreeSet<ResourceId>,
    grants: BTreeMap<ResourceId, BTreeSet<MclCapabilityOwner>>,
}

impl MclCapabilityStore {
    pub fn default_resources(&self) -> &BTreeSet<ResourceId> {
        &self.default
    }

    pub fn visible_resources(&self) -> impl Iterator<Item = &ResourceId> {
        self.grants
            .iter()
            .filter(|(_, owners)| !owners.is_empty())
            .map(|(resource, _)| resource)
    }

    pub fn is_visible(&self, resource_id: &ResourceId) -> bool {
        self.grants
            .get(resource_id)
            .is_some_and(|set| !set.is_empty())
    }

    fn grant(&mut self, owner: MclCapabilityOwner, resource_id: ResourceId) {
        self.grants.entry(resource_id).or_default().insert(owner);
    }

    fn revoke(&mut self, owner: &MclCapabilityOwner, resource_id: &ResourceId) {
        let remove_resource = self.grants.get_mut(resource_id).is_some_and(|owners| {
            owners.remove(owner);
            owners.is_empty()
        });
        if remove_resource {
            self.grants.remove(resource_id);
        }
    }

    fn clear_owner(&mut self, owner: &MclCapabilityOwner) {
        self.grants.retain(|_, owners| {
            owners.remove(owner);
            !owners.is_empty()
        });
    }
}

#[derive(Clone, Debug)]
pub struct MclBlock {
    definition: MclBlockDefinition,
    items: Vec<MclContextItem>,
}

impl MclBlock {
    pub fn definition(&self) -> &MclBlockDefinition {
        &self.definition
    }

    pub fn items(&self) -> &[MclContextItem] {
        &self.items
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclToolExchangeState {
    Open,
    Closed,
    Interrupted,
}

#[derive(Clone, Debug)]
pub struct MclToolExchange {
    assistant: Message,
    responses: BTreeMap<String, Message>,
    state: MclToolExchangeState,
}

impl MclToolExchange {
    pub fn assistant(&self) -> &Message {
        &self.assistant
    }

    pub fn responses(&self) -> impl Iterator<Item = &Message> {
        let Message::Assistant { tool_calls, .. } = &self.assistant else {
            unreachable!("MclToolExchange always owns an Assistant message");
        };
        tool_calls
            .iter()
            .filter_map(|call| self.responses.get(&call.id))
    }

    pub fn state(&self) -> MclToolExchangeState {
        self.state
    }
}

#[derive(Clone, Debug)]
pub enum MclContextItem {
    Message(Message),
    ToolExchange(MclToolExchange),
}

#[derive(Clone, Debug, Default)]
pub struct MclContextStore {
    blocks: BTreeMap<String, MclBlock>,
}

impl MclContextStore {
    pub fn block(&self, name: &str) -> Option<&MclBlock> {
        self.blocks.get(name)
    }
}

#[derive(Clone, Debug)]
pub struct WorkflowMclInstance {
    id: WorkflowInstanceId,
    resource_id: ResourceId,
    program: Arc<MclProgram>,
    blocks: MclContextStore,
    pending_effects: BTreeSet<String>,
}

impl WorkflowMclInstance {
    pub fn id(&self) -> &WorkflowInstanceId {
        &self.id
    }

    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }

    pub fn program(&self) -> &MclProgram {
        &self.program
    }
}

#[derive(Clone)]
pub struct AgentMcl {
    base: Arc<MclProgram>,
    workflows: BTreeMap<WorkflowInstanceId, WorkflowMclInstance>,
    context: MclContextStore,
    capabilities: MclCapabilityStore,
    system_prompt: String,
    plan_hash: MclHash,
    plan_generation: u64,
}

impl AgentMcl {
    pub fn base(&self) -> &MclProgram {
        &self.base
    }

    pub fn workflows(&self) -> impl Iterator<Item = (&WorkflowInstanceId, &WorkflowMclInstance)> {
        self.workflows.iter()
    }

    pub fn context(&self) -> &MclContextStore {
        &self.context
    }

    pub fn capabilities(&self) -> &MclCapabilityStore {
        &self.capabilities
    }

    pub fn plan_hash(&self) -> &MclHash {
        &self.plan_hash
    }

    pub fn plan_generation(&self) -> u64 {
        self.plan_generation
    }
}

impl Component for AgentMcl {}

#[derive(Clone, Debug)]
pub enum MclRuntimeEvent {
    AgentCreated,
    UserMessage { entry: Message },
    AssistantMessage { entry: Message },
    ToolMessage { entry: Message },
    ToolBatchCompleted,
    ToolBatchFailed,
    InferenceFailed,
    TurnAborted,
    ResourceInjected { resource_id: ResourceId },
    ResourceRemoved { resource_id: ResourceId },
    WorkflowAttached { instance_id: WorkflowInstanceId },
    WorkflowDetaching { instance_id: WorkflowInstanceId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclEffect {
    ResolveResources {
        owner: MclCapabilityOwner,
        resources: Vec<ResourceId>,
    },
    RequestInference {
        request: String,
    },
    ExecuteTools {
        calls: Vec<ToolCall>,
    },
    FinishTurn,
}

#[derive(Clone, Debug)]
pub struct MclEffectsProduced {
    pub id: String,
    pub agent: Entity,
    pub effects: Vec<MclEffect>,
}

impl Event for MclEffectsProduced {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MclResourceAliasDeclared {
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: String,
}

impl Event for MclResourceAliasDeclared {}

#[derive(Clone, Debug)]
pub struct MclSnapshotProvenance {
    pub request: String,
    pub views: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ModelRequestSnapshot {
    pub plan_hash: MclHash,
    pub plan_generation: u64,
    pub base_program_hash: MclHash,
    pub workflow_program_hashes: BTreeMap<WorkflowInstanceId, MclHash>,
    pub system: String,
    pub messages: Vec<Message>,
    pub visible_resources: Vec<ResourceId>,
    pub provenance: MclSnapshotProvenance,
}

#[derive(Clone, Debug)]
pub struct AttachAgentMclRequest {
    pub base: Arc<MclProgram>,
    pub system_prompt: String,
    pub context_window_tokens: u64,
    pub restored_messages: Vec<Message>,
    pub default_visibility: BTreeSet<ResourceId>,
}

#[derive(Clone, Debug)]
pub struct AttachWorkflowMcl {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub project_root: PathBuf,
    pub image_root: PathBuf,
}

impl Event for AttachWorkflowMcl {}

#[derive(Clone, Debug)]
pub struct DetachWorkflowMcl {
    pub id: String,
    pub agent: Entity,
    pub instance_id: WorkflowInstanceId,
}

impl Event for DetachWorkflowMcl {}

#[derive(Clone, Debug)]
pub struct WorkflowMclAttached {
    pub id: String,
    pub agent: Entity,
    pub instance_id: WorkflowInstanceId,
    pub resource_id: ResourceId,
}

impl Event for WorkflowMclAttached {}

#[derive(Clone, Debug)]
pub struct WorkflowMclAttachFailed {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub error: MclError,
}

impl Event for WorkflowMclAttachFailed {}

#[derive(Clone, Debug)]
pub struct WorkflowMclDetached {
    pub id: String,
    pub agent: Entity,
    pub instance_id: WorkflowInstanceId,
    pub removed_resources: Vec<ResourceId>,
}

impl Event for WorkflowMclDetached {}

#[derive(Clone, Debug)]
pub struct WorkflowMclDetachFailed {
    pub id: String,
    pub agent: Entity,
    pub instance_id: WorkflowInstanceId,
    pub error: MclError,
}

impl Event for WorkflowMclDetachFailed {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MclErrorKind {
    InvalidResourceId,
    SourceReadFailed,
    SourceTooLarge,
    SourceInvalidUtf8,
    LuaFailed,
    ParseFailed,
    InvalidProgramKind,
    ImportCycle,
    ImportMissing,
    DuplicateName,
    TypeMismatch,
    WriteConflict,
    AgentMissing,
    AgentMclMissing,
    WorkflowMissing,
    WorkflowBusy,
    InvalidEvent,
    InvalidMessageSequence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MclError {
    kind: MclErrorKind,
    message: String,
}

impl MclError {
    pub fn new(kind: MclErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MAX_ERROR_BYTES {
            let mut end = MAX_ERROR_BYTES - 3;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push_str("...");
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> MclErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MclError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for MclError {}

pub fn read_mcl_source(root: &Path, resource_id: &ResourceId) -> Result<MclSource, MclError> {
    if resource_id.resource_type() != "mcl" {
        return Err(MclError::new(
            MclErrorKind::InvalidResourceId,
            "MCL resource ID must use type mcl",
        ));
    }
    let root = normalize_root(root)?;
    let path = root
        .join("mcl")
        .join(resource_id.scope())
        .join(resource_id.name())
        .join(resource_id.tag())
        .join("main.mcl");
    let metadata = fs::metadata(&path).map_err(|error| {
        MclError::new(
            MclErrorKind::SourceReadFailed,
            format!("MCL source metadata could not be read: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(MclError::new(
            MclErrorKind::SourceReadFailed,
            "MCL source is not a regular file",
        ));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(MclError::new(
            MclErrorKind::SourceTooLarge,
            "MCL source exceeds the size limit",
        ));
    }
    let bytes = fs::read(&path).map_err(|error| {
        MclError::new(
            MclErrorKind::SourceReadFailed,
            format!("MCL source could not be read: {error}"),
        )
    })?;
    let source = String::from_utf8(bytes).map_err(|_| {
        MclError::new(
            MclErrorKind::SourceInvalidUtf8,
            "MCL source is not valid UTF-8",
        )
    })?;
    Ok(MclSource::new(resource_id.clone(), source, path))
}

pub fn load_mcl_program(
    roots: &[PathBuf],
    resource_id: &ResourceId,
    expected: MclProgramKind,
) -> Result<Arc<MclProgram>, MclError> {
    let mut sources = BTreeMap::new();
    let mut visiting = HashSet::new();
    load_source_graph(roots, resource_id, &mut sources, &mut visiting)?;
    let root = sources
        .remove(resource_id)
        .ok_or_else(|| MclError::new(MclErrorKind::ImportMissing, "root MCL source is missing"))?;
    let program = compile_mcl(MclCompileRequest {
        root,
        dependencies: sources,
    })?;
    if program.kind() != expected {
        return Err(MclError::new(
            MclErrorKind::InvalidProgramKind,
            format!(
                "MCL resource has {:?} kind, expected {expected:?}",
                program.kind()
            ),
        ));
    }
    Ok(program)
}

/// Loads a Base MCL whose source file is owned by an AgentImage rather than a
/// separately addressable MCL resource directory. Imports still resolve using
/// the normal MCL resource roots.
pub fn load_mcl_program_from_path(
    roots: &[PathBuf],
    resource_id: &ResourceId,
    path: &Path,
    expected: MclProgramKind,
) -> Result<Arc<MclProgram>, MclError> {
    if resource_id.resource_type() != "mcl" {
        return Err(MclError::new(
            MclErrorKind::InvalidResourceId,
            "MCL resource ID must use type mcl",
        ));
    }
    let metadata = fs::metadata(path).map_err(|error| {
        MclError::new(
            MclErrorKind::SourceReadFailed,
            format!("MCL source metadata could not be read: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(MclError::new(
            MclErrorKind::SourceReadFailed,
            "MCL source is not a regular file",
        ));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(MclError::new(
            MclErrorKind::SourceTooLarge,
            "MCL source exceeds the size limit",
        ));
    }
    let source = String::from_utf8(fs::read(path).map_err(|error| {
        MclError::new(
            MclErrorKind::SourceReadFailed,
            format!("MCL source could not be read: {error}"),
        )
    })?)
    .map_err(|_| {
        MclError::new(
            MclErrorKind::SourceInvalidUtf8,
            "MCL source is not valid UTF-8",
        )
    })?;
    let root = MclSource::new(resource_id.clone(), source, path.to_path_buf());
    let mut sources = BTreeMap::new();
    let mut visiting = HashSet::new();
    visiting.insert(resource_id.clone());
    if !root.source().contains("handle(") {
        for import in syntax::scan_imports(root.source())? {
            load_source_graph(roots, &import, &mut sources, &mut visiting)?;
        }
    }
    sources.insert(resource_id.clone(), root);
    let program = compile_mcl(MclCompileRequest {
        root: sources.remove(resource_id).ok_or_else(|| {
            MclError::new(MclErrorKind::ImportMissing, "root MCL source is missing")
        })?,
        dependencies: sources,
    })?;
    if program.kind() != expected {
        return Err(MclError::new(
            MclErrorKind::InvalidProgramKind,
            format!(
                "MCL resource has {:?} kind, expected {expected:?}",
                program.kind()
            ),
        ));
    }
    Ok(program)
}

fn load_source_graph(
    roots: &[PathBuf],
    resource_id: &ResourceId,
    sources: &mut BTreeMap<ResourceId, MclSource>,
    visiting: &mut HashSet<ResourceId>,
) -> Result<(), MclError> {
    if sources.contains_key(resource_id) {
        return Ok(());
    }
    if !visiting.insert(resource_id.clone()) {
        return Err(MclError::new(
            MclErrorKind::ImportCycle,
            format!("MCL import cycle contains {resource_id}"),
        ));
    }
    let mut source = None;
    for root in roots {
        match read_mcl_source(root, resource_id) {
            Ok(found) => {
                source = Some(found);
                break;
            }
            Err(error) if error.kind() == MclErrorKind::SourceReadFailed => {}
            Err(error) => return Err(error),
        }
    }
    let source = source.ok_or_else(|| {
        MclError::new(
            MclErrorKind::ImportMissing,
            format!("MCL resource {resource_id} was not found"),
        )
    })?;
    for import in syntax::scan_imports(source.source())? {
        load_source_graph(roots, &import, sources, visiting)?;
    }
    visiting.remove(resource_id);
    sources.insert(resource_id.clone(), source);
    Ok(())
}

pub fn compile_mcl(request: MclCompileRequest) -> Result<Arc<MclProgram>, MclError> {
    // Base Drivers are executable Lua, not legacy MCL handler graphs.  Keep
    // their source as-is; the Driver thread is the interpreter.  This branch
    // exists only to preserve the source-loading API while callers migrate to
    // MclDriverSource.
    if request.root.source().contains("handle(") {
        let source_hash = MclHash::digest([request.root.source().as_bytes()]);
        return Ok(Arc::new(MclProgram {
            resource_id: request.root.resource_id.clone(),
            source: Arc::clone(&request.root.source),
            origin: Arc::clone(&request.root.origin),
            kind: MclProgramKind::Base,
            name: request.root.resource_id.name().to_owned(),
            source_hash: source_hash.clone(),
            plan_hash: source_hash,
            imports: Vec::new(),
            blocks: Vec::new(),
            views: Vec::new(),
            requests: Vec::new(),
            handlers: Vec::new(),
        }));
    }
    let root_ast = syntax::parse(&request.root)?;
    let mut visiting = BTreeSet::from([request.root.resource_id.clone()]);
    let mut visited = BTreeSet::new();
    let mut imports = Vec::new();
    let mut imported_asts = Vec::new();
    for resource_id in &root_ast.imports {
        collect_import_ast(
            resource_id,
            &request.dependencies,
            &mut visiting,
            &mut visited,
            &mut imports,
            &mut imported_asts,
        )?;
    }
    let mut blocks = Vec::new();
    let mut views = Vec::new();
    let mut requests = Vec::new();
    let mut handlers = Vec::new();
    for ast in imported_asts.iter().chain(std::iter::once(&root_ast)) {
        blocks.extend(ast.blocks.iter().cloned());
        views.extend(ast.views.iter().cloned());
        requests.extend(ast.requests.iter().cloned());
        handlers.extend(ast.handlers.iter().cloned());
    }
    syntax::validate(&blocks, &views, &requests, &handlers)?;
    let source_hash = MclHash::digest([request.root.source().as_bytes()]);
    let mut plan_parts = vec![COMPILER_VERSION.as_bytes(), source_hash.as_str().as_bytes()];
    for import in &imports {
        plan_parts.push(import.source_hash.as_str().as_bytes());
    }
    let plan_hash = MclHash::digest(plan_parts);
    Ok(Arc::new(MclProgram {
        resource_id: request.root.resource_id.clone(),
        source: Arc::clone(&request.root.source),
        origin: Arc::clone(&request.root.origin),
        kind: root_ast.kind,
        name: root_ast.name,
        source_hash,
        plan_hash,
        imports,
        blocks,
        views,
        requests,
        handlers,
    }))
}

fn collect_import_ast(
    resource_id: &ResourceId,
    dependencies: &BTreeMap<ResourceId, MclSource>,
    visiting: &mut BTreeSet<ResourceId>,
    visited: &mut BTreeSet<ResourceId>,
    imports: &mut Vec<MclProgramDependency>,
    imported_asts: &mut Vec<syntax::MclAst>,
) -> Result<(), MclError> {
    if visited.contains(resource_id) {
        return Ok(());
    }
    if !visiting.insert(resource_id.clone()) {
        return Err(MclError::new(
            MclErrorKind::ImportCycle,
            format!("MCL import cycle contains {resource_id}"),
        ));
    }
    let source = dependencies.get(resource_id).ok_or_else(|| {
        MclError::new(
            MclErrorKind::ImportMissing,
            format!("MCL import {resource_id} is missing"),
        )
    })?;
    let ast = syntax::parse(source)?;
    if ast.kind != MclProgramKind::Module {
        return Err(MclError::new(
            MclErrorKind::InvalidProgramKind,
            format!("MCL import {resource_id} is not a module"),
        ));
    }
    for dependency in &ast.imports {
        collect_import_ast(
            dependency,
            dependencies,
            visiting,
            visited,
            imports,
            imported_asts,
        )?;
    }
    visiting.remove(resource_id);
    visited.insert(resource_id.clone());
    imports.push(MclProgramDependency {
        resource_id: resource_id.clone(),
        source_hash: MclHash::digest([source.source().as_bytes()]),
        origin: Arc::clone(&source.origin),
    });
    imported_asts.push(ast);
    Ok(())
}

fn normalize_root(root: &Path) -> Result<PathBuf, MclError> {
    if !root.is_absolute() {
        return Err(MclError::new(
            MclErrorKind::SourceReadFailed,
            "MCL resource root must be absolute",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in root.components() {
        match component {
            PathComponent::Prefix(_) | PathComponent::RootDir | PathComponent::Normal(_) => {
                normalized.push(component.as_os_str())
            }
            PathComponent::CurDir => {}
            PathComponent::ParentDir => {
                return Err(MclError::new(
                    MclErrorKind::SourceReadFailed,
                    "MCL resource root cannot contain parent traversal",
                ));
            }
        }
    }
    Ok(normalized)
}

pub struct MclPlugin {
    home_root: PathBuf,
    schedule: String,
}

impl MclPlugin {
    pub fn open(home_root: impl Into<PathBuf>) -> Result<Self, MclError> {
        Ok(Self {
            home_root: normalize_root(&home_root.into())?,
            schedule: RuntimePlugin::UPDATE.to_owned(),
        })
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Plugin for MclPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            panic!("RuntimePlugin is not installed");
        }
        if app.world().contains_resource::<MclPluginInstalled>() {
            panic!("MclPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("MclPlugin schedule does not exist");
        }
        app.world_mut().insert_resource(MclPluginInstalled);
        app.world_mut()
            .insert_resource(runtime::MclDriverMailboxes::default());
        app.world_mut().insert_resource(MclRuntime {
            home_root: Arc::new(self.home_root),
        });
        app.add_system(&self.schedule, runtime::start_driver_system)
            .add_system(&self.schedule, runtime::mcl_command_system)
            .add_system(&self.schedule, runtime::workflow_control_system);
    }
}

pub struct MclPluginInstalled;
impl Resource for MclPluginInstalled {}

struct MclRuntime {
    home_root: Arc<PathBuf>,
}

impl Resource for MclRuntime {}
