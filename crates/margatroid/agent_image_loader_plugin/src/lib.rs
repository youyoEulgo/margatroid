use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{AppAsyncExt, AsyncTaskError, WorldAsyncExt};
use core_plugin::{App, Component as MecsComponent, Entity, Event, Plugin, Resource, World};
use futures_util::FutureExt;
use margatroid_types::ResourceId;
use mcl_plugin::{load_mcl_program_from_path, MclProgram, MclProgramKind};
use resource_id_plugin::ResourceIdPluginInstalled;
use resource_id_plugin::WorldResourceIdExt;
use serde::Deserialize;
use tokio::io::AsyncReadExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentImageLoadErrorKind {
    InvalidRoot,
    InvalidRequest,
    NotFound,
    InvalidLayout,
    SymlinkNotAllowed,
    LimitExceeded,
    SourceChanged,
    ManifestReadFailed,
    ManifestDecodeFailed,
    UnsupportedSchema,
    InvalidModelConfig,
    SoulReadFailed,
    SoulInvalidUtf8,
    InvalidResourceName,
    BaseMclLoadFailed,
    TaskPanicked,
}

#[derive(Clone, Debug)]
pub struct AgentImageLoadError {
    kind: AgentImageLoadErrorKind,
    message: String,
}

impl AgentImageLoadError {
    fn new(kind: AgentImageLoadErrorKind, message: impl Into<String>) -> Self {
        const MAX_MESSAGE_BYTES: usize = 512;

        let mut message = message.into();
        if message.len() > MAX_MESSAGE_BYTES {
            let mut boundary = MAX_MESSAGE_BYTES - 3;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str("...");
        }
        Self { kind, message }
    }

    fn invalid_root(message: impl Into<String>) -> Self {
        Self::new(AgentImageLoadErrorKind::InvalidRoot, message)
    }

    pub fn kind(&self) -> AgentImageLoadErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AgentImageLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for AgentImageLoadError {}

pub struct LoadAgentImage {
    pub id: String,
    pub reference: ResourceId,
}

impl Event for LoadAgentImage {}

pub struct LoadAgentImageResult {
    pub id: String,
    pub reference: ResourceId,
    pub result: Result<Entity, AgentImageLoadError>,
}

impl Event for LoadAgentImageResult {}

#[derive(Clone, Debug)]
pub struct AgentImageSoul {
    content: Arc<str>,
}

impl AgentImageSoul {
    pub fn as_str(&self) -> &str {
        &self.content
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageBaseDriver {
    program: Arc<MclProgram>,
}

impl AgentImageBaseDriver {
    pub fn program(&self) -> &Arc<MclProgram> {
        &self.program
    }
}

pub type AgentImageBaseMcl = AgentImageBaseDriver;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentImageDependency {
    resource_id: ResourceId,
    source: Option<Arc<str>>,
}

impl AgentImageDependency {
    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageDependencies {
    entries: Arc<[AgentImageDependency]>,
}

impl AgentImageDependencies {
    pub fn entries(&self) -> &[AgentImageDependency] {
        &self.entries
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageModelParameters {
    temperature: Option<f32>,
    max_output_tokens: Option<u32>,
    top_p: Option<f32>,
    stop: Arc<[String]>,
}

impl AgentImageModelParameters {
    pub fn temperature(&self) -> Option<f32> {
        self.temperature
    }

    pub fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }

    pub fn top_p(&self) -> Option<f32> {
        self.top_p
    }

    pub fn stop(&self) -> &[String] {
        &self.stop
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageModelConfig {
    model: Arc<str>,
    parameters: AgentImageModelParameters,
}

impl AgentImageModelConfig {
    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn parameters(&self) -> &AgentImageModelParameters {
        &self.parameters
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageDefaultVisibility {
    resources: BTreeSet<ResourceId>,
}

impl AgentImageDefaultVisibility {
    pub fn resources(&self) -> impl Iterator<Item = &ResourceId> + '_ {
        self.resources.iter()
    }
}

#[derive(Clone, Debug)]
pub struct AgentImage {
    base_driver: AgentImageBaseDriver,
    dependencies: AgentImageDependencies,
    model: AgentImageModelConfig,
    default_visibility: AgentImageDefaultVisibility,
}

impl AgentImage {
    pub fn base_driver(&self) -> &AgentImageBaseDriver {
        &self.base_driver
    }

    pub fn dependencies(&self) -> &[AgentImageDependency] {
        self.dependencies.entries()
    }

    pub fn model(&self) -> &AgentImageModelConfig {
        &self.model
    }

    pub fn default_visibility(&self) -> impl Iterator<Item = &ResourceId> + '_ {
        self.default_visibility.resources()
    }
}

impl MecsComponent for AgentImage {}

pub struct AgentImageLoaderPlugin {
    root: PathBuf,
    schedule: String,
    limits: AgentImageLoaderLimits,
}

impl AgentImageLoaderPlugin {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, AgentImageLoadError> {
        let root = normalize_root(root.into())?;
        ensure_root(&root)?;
        Ok(Self {
            root,
            schedule: RuntimePlugin::PRE_UPDATE.to_owned(),
            limits: AgentImageLoaderLimits::default(),
        })
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Plugin for AgentImageLoaderPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            panic!("RuntimePlugin is not installed");
        }
        if !app
            .world()
            .contains_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
        {
            panic!("AsyncRuntimePlugin is not installed");
        }
        if !app.world().contains_resource::<ResourceIdPluginInstalled>() {
            panic!("ResourceIdPlugin is not installed");
        }
        if app
            .world()
            .contains_resource::<AgentImageLoaderPluginInstalled>()
        {
            panic!("AgentImageLoaderPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("AgentImageLoaderPlugin schedule does not exist");
        }
        let schedule = self.schedule;
        app.world_mut()
            .insert_resource(AgentImageLoaderPluginInstalled);
        app.world_mut().insert_resource(AgentImageLoaderState {
            root: Arc::new(self.root),
            limits: self.limits,
            pending: HashMap::new(),
        });
        app.add_system(&schedule, prepare_agent_image_load_system)
            .add_async_system(&schedule, read_agent_image)
            .add_system(&schedule, apply_agent_image_load_system);
    }
}

pub struct AgentImageLoaderPluginInstalled;

impl Resource for AgentImageLoaderPluginInstalled {}

pub(crate) struct AgentImageLoaderState {
    root: Arc<PathBuf>,
    limits: AgentImageLoaderLimits,
    pending: HashMap<ResourceId, Vec<String>>,
}

impl Resource for AgentImageLoaderState {}

#[derive(Deserialize)]
struct AgentImageManifest {
    schema_version: u32,
    inference: AgentImageModelDocument,
    #[serde(default)]
    dependencies: Vec<AgentImageDependencyDocument>,
}

#[derive(Deserialize)]
struct AgentImageDependencyDocument {
    id: String,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
struct AgentImageModelDocument {
    model: String,
    temperature: Option<f32>,
    max_output_tokens: Option<u32>,
    top_p: Option<f32>,
    #[serde(default)]
    stop: Vec<String>,
}

#[derive(Clone, Copy)]
struct AgentImageLoaderLimits {
    max_manifest_bytes: u64,
    max_soul_bytes: u64,
    max_model_id_bytes: usize,
    max_stop_sequences: usize,
    max_stop_sequence_bytes: usize,
}

impl Default for AgentImageLoaderLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 64 * 1024,
            max_soul_bytes: 1024 * 1024,
            max_model_id_bytes: 1024,
            max_stop_sequences: 128,
            max_stop_sequence_bytes: 4096,
        }
    }
}

struct AgentImageReadTask {
    reference: ResourceId,
    root: Arc<PathBuf>,
    limits: AgentImageLoaderLimits,
}

impl Event for AgentImageReadTask {}

struct PreparedAgentImage {
    reference: ResourceId,
    base_driver: AgentImageBaseDriver,
    dependencies: AgentImageDependencies,
    model: AgentImageModelConfig,
    default_visibility: AgentImageDefaultVisibility,
}

fn parse_default_visibility(
    source: &str,
    dependencies: &[AgentImageDependency],
) -> Result<BTreeSet<ResourceId>, AgentImageLoadError> {
    let mut aliases = HashMap::new();
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("mcl_command(\"IMPORT ") else {
            continue;
        };
        let Some((resource, alias)) = rest.split_once(" AS ") else {
            continue;
        };
        let resource = resource.trim();
        let alias = alias.trim().trim_end_matches("\")");
        if ResourceId::parse(resource).is_ok() {
            aliases.insert(alias.to_owned(), resource.to_owned());
        }
    }
    let Some(inject) = source
        .lines()
        .map(str::trim)
        .find(|line| line.contains("INJECT") && line.contains("TO tool_default"))
    else {
        return Ok(BTreeSet::new());
    };
    let names = inject
        .split("INJECT")
        .nth(1)
        .and_then(|value| value.split("TO tool_default").next())
        .unwrap_or_default();
    let mut result = BTreeSet::new();
    for alias in names.split(',').map(str::trim) {
        let Some(resource) = aliases.get(alias) else {
            continue;
        };
        let resource = ResourceId::parse(resource).map_err(|_| {
            AgentImageLoadError::new(
                AgentImageLoadErrorKind::InvalidResourceName,
                "Base Driver visibility references an invalid resource",
            )
        })?;
        if dependencies
            .iter()
            .any(|dependency| dependency.resource_id == resource)
        {
            result.insert(resource);
        }
    }
    Ok(result)
}

struct AgentImageReadPayload {
    reference: ResourceId,
    result: Result<PreparedAgentImage, AgentImageLoadError>,
}

struct AgentImageReadOutput {
    payload: Mutex<Option<AgentImageReadPayload>>,
}

impl AgentImageReadOutput {
    fn new(payload: AgentImageReadPayload) -> Self {
        Self {
            payload: Mutex::new(Some(payload)),
        }
    }

    fn take(&self) -> Option<AgentImageReadPayload> {
        self.payload
            .lock()
            .expect("agent image read output lock poisoned")
            .take()
    }
}

struct AgentImageTaskError {
    source: AsyncTaskError,
}

impl From<AsyncTaskError> for AgentImageTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DirectoryEntryKind {
    File,
    Directory,
}

#[derive(PartialEq, Eq)]
struct DirectoryEntrySignature {
    name: OsString,
    kind: DirectoryEntryKind,
}

#[derive(PartialEq, Eq)]
struct DirectorySignature {
    entries: Vec<DirectoryEntrySignature>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileSignature {
    length: u64,
    modified: Option<SystemTime>,
}

fn prepare_agent_image_load_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadAgentImage>()
        .into_iter()
        .map(|request| (request.id.clone(), request.reference.clone()))
        .collect::<Vec<_>>();

    for (id, reference) in requests {
        if id.is_empty() {
            world.send_event(LoadAgentImageResult {
                id,
                reference,
                result: Err(AgentImageLoadError::new(
                    AgentImageLoadErrorKind::InvalidRequest,
                    "request id cannot be empty",
                )),
            });
            continue;
        }

        let task = {
            let state = world
                .get_resource_mut::<AgentImageLoaderState>()
                .expect("AgentImageLoaderPlugin is not installed");
            if let Some(waiting) = state.pending.get_mut(&reference) {
                waiting.push(id);
                None
            } else {
                state.pending.insert(reference.clone(), vec![id]);
                Some(AgentImageReadTask {
                    reference,
                    root: Arc::clone(&state.root),
                    limits: state.limits,
                })
            }
        };
        if let Some(task) = task {
            world.send_async_event(task);
        }
    }
}

async fn read_agent_image(
    task: AgentImageReadTask,
) -> Result<AgentImageReadOutput, AgentImageTaskError> {
    let reference = task.reference.clone();
    let result = std::panic::AssertUnwindSafe(read_agent_image_inner(task))
        .catch_unwind()
        .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::TaskPanicked,
            "agent image loader task panicked",
        )),
    };
    Ok(AgentImageReadOutput::new(AgentImageReadPayload {
        reference,
        result,
    }))
}

async fn read_agent_image_inner(
    task: AgentImageReadTask,
) -> Result<PreparedAgentImage, AgentImageLoadError> {
    let image_root = resolve_image_root(&task.root, &task.reference).await?;
    let layout_before = validate_image_layout(&image_root).await?;

    let manifest_path = image_root.join("agent.toml");
    let soul_path = image_root.join("SOUL.md");
    let (manifest_bytes, manifest_signature) = read_bounded(
        &manifest_path,
        task.limits.max_manifest_bytes,
        AgentImageLoadErrorKind::ManifestReadFailed,
    )
    .await?;
    let manifest_source = std::str::from_utf8(&manifest_bytes).map_err(|_| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::ManifestDecodeFailed,
            "agent.toml is not valid UTF-8",
        )
    })?;
    let manifest: AgentImageManifest = toml::from_str(manifest_source).map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::ManifestDecodeFailed,
            format!("invalid agent.toml: {error}"),
        )
    })?;
    if manifest.schema_version != 1 {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::UnsupportedSchema,
            "agent.toml schema_version must be 1",
        ));
    }
    validate_model_document(&manifest.inference, &task.limits)?;
    let dependencies = manifest
        .dependencies
        .into_iter()
        .map(|dependency| {
            let resource_id = ResourceId::parse(&dependency.id).map_err(|_| {
                AgentImageLoadError::new(
                    AgentImageLoadErrorKind::InvalidResourceName,
                    "agent image dependency ID is invalid",
                )
            })?;
            if dependency.source.as_deref().is_some_and(|source| {
                source.trim().is_empty() || source.chars().any(char::is_control)
            }) {
                return Err(AgentImageLoadError::new(
                    AgentImageLoadErrorKind::InvalidResourceName,
                    "agent image dependency source is empty or contains control characters",
                ));
            }
            Ok(AgentImageDependency {
                resource_id,
                source: dependency.source.map(Arc::from),
            })
        })
        .collect::<Result<Arc<[_]>, _>>()?;
    let base_driver_id = ResourceId::new(
        "mcl",
        task.reference.scope(),
        task.reference.name(),
        Some(task.reference.tag()),
    )
    .map_err(|_| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::BaseMclLoadFailed,
            "AgentImage reference cannot derive a Base Driver resource ID",
        )
    })?;
    let base_driver = load_mcl_program_from_path(
        std::slice::from_ref(&image_root),
        &base_driver_id,
        &image_root.join("base.lua"),
        MclProgramKind::Base,
    )
    .map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::BaseMclLoadFailed,
            format!("Base Lua could not be loaded: {error}"),
        )
    })?;

    let (soul_bytes, soul_signature) = read_bounded(
        &soul_path,
        task.limits.max_soul_bytes,
        AgentImageLoadErrorKind::SoulReadFailed,
    )
    .await?;
    let soul = String::from_utf8(soul_bytes).map_err(|_| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::SoulInvalidUtf8,
            "SOUL.md is not valid UTF-8",
        )
    })?;
    if soul.trim().is_empty() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SoulReadFailed,
            "SOUL.md cannot be empty",
        ));
    }
    let _ = soul;
    let layout_after = validate_image_layout(&image_root).await?;
    if layout_before != layout_after
        || manifest_signature
            != file_signature(&manifest_path, AgentImageLoadErrorKind::ManifestReadFailed).await?
        || soul_signature
            != file_signature(&soul_path, AgentImageLoadErrorKind::SoulReadFailed).await?
    {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SourceChanged,
            "agent image changed while it was being read",
        ));
    }

    let resources = parse_default_visibility(
        &tokio::fs::read_to_string(image_root.join("base.lua"))
            .await
            .map_err(|_| {
                AgentImageLoadError::new(
                    AgentImageLoadErrorKind::BaseMclLoadFailed,
                    "Base Driver could not be read",
                )
            })?,
        &dependencies,
    )?;
    let inference = manifest.inference;
    Ok(PreparedAgentImage {
        reference: task.reference,
        base_driver: AgentImageBaseDriver {
            program: base_driver,
        },
        dependencies: AgentImageDependencies {
            entries: dependencies,
        },
        model: AgentImageModelConfig {
            model: Arc::from(inference.model),
            parameters: AgentImageModelParameters {
                temperature: inference.temperature,
                max_output_tokens: inference.max_output_tokens,
                top_p: inference.top_p,
                stop: Arc::from(inference.stop),
            },
        },
        default_visibility: AgentImageDefaultVisibility { resources },
    })
}

fn validate_model_document(
    document: &AgentImageModelDocument,
    limits: &AgentImageLoaderLimits,
) -> Result<(), AgentImageLoadError> {
    if document.model.is_empty()
        || document.model.len() > limits.max_model_id_bytes
        || document.model.chars().any(char::is_control)
    {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidModelConfig,
            "model id is empty, too large, or contains a control character",
        ));
    }
    if document.stop.len() > limits.max_stop_sequences
        || document
            .stop
            .iter()
            .any(|sequence| sequence.len() > limits.max_stop_sequence_bytes)
    {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::LimitExceeded,
            "stop sequences exceed the configured loading limit",
        ));
    }
    Ok(())
}

fn apply_agent_image_load_system(world: &mut World) {
    let mut payloads = Vec::new();
    for output in world.event_reader::<Result<AgentImageReadOutput, AgentImageTaskError>>() {
        match output {
            Ok(output) => payloads.extend(output.take()),
            Err(error) => {
                tracing::error!(error = %error.source, "agent image loader async task stopped");
            }
        }
    }

    for payload in payloads {
        apply_agent_image_payload(world, payload);
    }
}

fn apply_agent_image_payload(world: &mut World, payload: AgentImageReadPayload) {
    let waiting = world
        .get_resource_mut::<AgentImageLoaderState>()
        .expect("AgentImageLoaderPlugin is not installed")
        .pending
        .remove(&payload.reference);
    let Some(waiting) = waiting else {
        tracing::error!(reference = %payload.reference, "agent image result has no pending request");
        return;
    };

    match payload.result {
        Ok(prepared) => {
            debug_assert_eq!(prepared.reference, payload.reference);
            let entity = world
                .entity_by_resource_id(&payload.reference)
                .ok()
                .filter(|entity| world.is_alive(*entity))
                .unwrap_or_else(|| world.spawn());
            let PreparedAgentImage {
                base_driver,
                dependencies,
                model,
                default_visibility,
                ..
            } = prepared;
            assert!(world.insert_component(entity, payload.reference.clone()));
            assert!(world.insert_component(
                entity,
                AgentImage {
                    base_driver,
                    dependencies,
                    model,
                    default_visibility,
                },
            ));
            for id in waiting {
                world.send_event(LoadAgentImageResult {
                    id,
                    reference: payload.reference.clone(),
                    result: Ok(entity),
                });
            }
        }
        Err(error) => {
            for id in waiting {
                world.send_event(LoadAgentImageResult {
                    id,
                    reference: payload.reference.clone(),
                    result: Err(error.clone()),
                });
            }
        }
    }
}

async fn resolve_image_root(
    root: &Path,
    reference: &ResourceId,
) -> Result<PathBuf, AgentImageLoadError> {
    if reference.resource_type() != "image" {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidResourceName,
            "agent image resource type must be image",
        ));
    }
    let Some(root) = check_directory(root, true).await? else {
        return Err(AgentImageLoadError::invalid_root(
            "agent image library root does not exist",
        ));
    };
    let mut current = root;
    for part in [reference.scope(), reference.name(), reference.tag()] {
        let candidate = current.join(part);
        let Some(candidate) = check_directory(&candidate, false).await? else {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::NotFound,
                format!("agent image `{reference}` was not found"),
            ));
        };
        current = candidate;
    }
    Ok(current)
}

async fn validate_image_layout(root: &Path) -> Result<DirectorySignature, AgentImageLoadError> {
    let signature = directory_signature(root, 6).await?;
    let mut manifest = false;
    let mut soul = false;
    for entry in &signature.entries {
        match (entry.name.to_str(), entry.kind) {
            (Some("agent.toml"), DirectoryEntryKind::File) => manifest = true,
            (Some("SOUL.md"), DirectoryEntryKind::File) => soul = true,
            (Some("COMPACT.md"), DirectoryEntryKind::File) => {}
            (Some("base.lua"), DirectoryEntryKind::File) => {}
            _ => {
                return Err(AgentImageLoadError::new(
                    AgentImageLoadErrorKind::InvalidLayout,
                    "agent image contains an unknown entry or invalid entry type",
                ))
            }
        }
    }
    let base_driver = signature.entries.iter().any(|entry| {
        entry.name.to_str() == Some("base.lua") && entry.kind == DirectoryEntryKind::File
    });
    if !manifest || !soul || !base_driver {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            "agent image requires agent.toml, SOUL.md, and base.lua",
        ));
    }
    Ok(signature)
}

fn normalize_root(root: PathBuf) -> Result<PathBuf, AgentImageLoadError> {
    if !root.is_absolute() || has_parent(&root) {
        return Err(AgentImageLoadError::invalid_root(
            "agent image root must be an absolute path without parent traversal",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in root.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => unreachable!("parent components were rejected"),
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn ensure_root(root: &Path) -> Result<(), AgentImageLoadError> {
    std::fs::create_dir_all(root).map_err(|error| {
        AgentImageLoadError::invalid_root(format!("cannot create agent image root: {error}"))
    })?;
    let metadata = std::fs::symlink_metadata(root).map_err(|error| {
        AgentImageLoadError::invalid_root(format!("cannot inspect agent image root: {error}"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AgentImageLoadError::invalid_root(
            "agent image root must be a directory and cannot be a symlink",
        ));
    }
    Ok(())
}

async fn check_directory(path: &Path, root: bool) -> Result<Option<PathBuf>, AgentImageLoadError> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AgentImageLoadError::new(
                if root {
                    AgentImageLoadErrorKind::InvalidRoot
                } else {
                    AgentImageLoadErrorKind::InvalidLayout
                },
                format!("cannot inspect agent image directory: {error}"),
            ))
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SymlinkNotAllowed,
            "agent image path cannot contain symlinks",
        ));
    }
    if !metadata.is_dir() {
        return Err(AgentImageLoadError::new(
            if root {
                AgentImageLoadErrorKind::InvalidRoot
            } else {
                AgentImageLoadErrorKind::InvalidLayout
            },
            "agent image path must be a directory",
        ));
    }
    Ok(Some(path.to_path_buf()))
}

async fn directory_signature(
    path: &Path,
    maximum_entries: usize,
) -> Result<DirectorySignature, AgentImageLoadError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            format!("cannot inspect agent image directory: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SymlinkNotAllowed,
            "agent image directory cannot be a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            "agent image path must be a directory",
        ));
    }

    let mut directory = tokio::fs::read_dir(path).await.map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            format!("cannot read agent image directory: {error}"),
        )
    })?;
    let mut entries = Vec::new();
    while let Some(entry) = directory.next_entry().await.map_err(|error| {
        AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            format!("cannot read agent image directory entry: {error}"),
        )
    })? {
        let metadata = tokio::fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| {
                AgentImageLoadError::new(
                    AgentImageLoadErrorKind::SourceChanged,
                    format!("agent image directory changed while reading: {error}"),
                )
            })?;
        if metadata.file_type().is_symlink() {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::SymlinkNotAllowed,
                "agent image directory cannot contain symlinks",
            ));
        }
        let kind = if metadata.is_file() {
            DirectoryEntryKind::File
        } else if metadata.is_dir() {
            DirectoryEntryKind::Directory
        } else {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::InvalidLayout,
                "agent image directory cannot contain special files",
            ));
        };
        entries.push(DirectoryEntrySignature {
            name: entry.file_name(),
            kind,
        });
        if entries.len() > maximum_entries {
            return Err(AgentImageLoadError::new(
                AgentImageLoadErrorKind::LimitExceeded,
                "agent image directory entry count exceeds the configured limit",
            ));
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(DirectorySignature { entries })
}

async fn read_bounded(
    path: &Path,
    maximum: u64,
    read_error: AgentImageLoadErrorKind,
) -> Result<(Vec<u8>, FileSignature), AgentImageLoadError> {
    let before = file_signature(path, read_error).await?;
    if before.length > maximum {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::LimitExceeded,
            "agent image file exceeds the configured limit",
        ));
    }
    let file = tokio::fs::File::open(path).await.map_err(|error| {
        AgentImageLoadError::new(read_error, format!("cannot open agent image file: {error}"))
    })?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| {
            AgentImageLoadError::new(read_error, format!("cannot read agent image file: {error}"))
        })?;
    if bytes.len() as u64 > maximum {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::LimitExceeded,
            "agent image file exceeds the configured limit",
        ));
    }
    let after = file_signature(path, read_error).await?;
    if before != after || bytes.len() as u64 != before.length {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SourceChanged,
            "agent image file changed while it was being read",
        ));
    }
    Ok((bytes, before))
}

async fn file_signature(
    path: &Path,
    read_error: AgentImageLoadErrorKind,
) -> Result<FileSignature, AgentImageLoadError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        AgentImageLoadError::new(
            read_error,
            format!("cannot inspect agent image file: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::SymlinkNotAllowed,
            "agent image file cannot be a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(AgentImageLoadError::new(
            AgentImageLoadErrorKind::InvalidLayout,
            "agent image file must be a regular file",
        ));
    }
    Ok(FileSignature {
        length: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn has_parent(path: &Path) -> bool {
    path.components()
        .any(|component| component == Component::ParentDir)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use core_plugin::App;
    use resource_id_plugin::ResourceIdPlugin;

    use super::*;

    static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "margatroid-agent-image-loader-{label}-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn image_root(library: &Path) -> PathBuf {
        library.join("local/coder/latest")
    }

    fn write_image(library: &Path, soul: &str) {
        let image = image_root(library);
        fs::create_dir_all(&image).unwrap();
        fs::write(
            image.join("agent.toml"),
            r#"schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]

[[dependencies]]
id = "skill:local/code-review:latest"

[[dependencies]]
id = "workflow:local/review:latest"
"#,
        )
        .unwrap();
        fs::write(image.join("SOUL.md"), soul).unwrap();
        fs::write(
            image.join("base.lua"),
            "mcl_command(\"IMPORT skill:local/code-review:latest AS review\")\nmcl_command(\"IMPORT workflow:local/review:latest AS workflow\")\nmcl_command(\"INJECT review, workflow TO tool_default FROM tool\")\nmcl_command(\"INJECT SELECT tool_default FROM tool COVER tool_dynamic FROM tool\")\n",
        )
        .unwrap();
    }

    fn app(library: &Path) -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ResourceIdPlugin)
            .add_plugin(AgentImageLoaderPlugin::open(library).unwrap());
        app
    }

    fn load(
        app: &mut App,
        id: &str,
        reference: &ResourceId,
    ) -> Result<Entity, AgentImageLoadError> {
        app.world().send_event(LoadAgentImage {
            id: id.to_owned(),
            reference: reference.clone(),
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(event) = app
                .world()
                .event_reader::<LoadAgentImageResult>()
                .into_iter()
                .find(|event| event.id == id)
            {
                return event.result.clone();
            }
            assert!(Instant::now() < deadline, "agent image load timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn loads_an_image_into_complete_read_only_components() {
        let library = unique_directory("components");
        write_image(&library, "You are a careful coder.\n");
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder").unwrap();

        let entity = load(&mut app, "load-components", &reference).unwrap();

        let identity = app.world().get_component::<ResourceId>(entity).unwrap();
        let image = app.world().get_component::<AgentImage>(entity).unwrap();

        assert_eq!(identity, &reference);
        assert_eq!(image.model().model(), "deepseek-v4-flash");
        assert_eq!(image.model().parameters().temperature(), Some(0.7));
        assert_eq!(image.model().parameters().max_output_tokens(), Some(8192));
        assert_eq!(image.model().parameters().top_p(), Some(0.9));
        assert_eq!(image.model().parameters().stop(), ["DONE"]);
        assert_eq!(
            image
                .default_visibility()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            [
                "skill:local/code-review:latest",
                "workflow:local/review:latest"
            ]
        );
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn concurrent_requests_share_one_read_and_entity() {
        let library = unique_directory("concurrent");
        write_image(&library, "Concurrent image.\n");
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder:latest").unwrap();
        for id in ["first", "second"] {
            app.world().send_event(LoadAgentImage {
                id: id.to_owned(),
                reference: reference.clone(),
            });
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        let entities = loop {
            app.tick();
            let results = app
                .world()
                .event_reader::<LoadAgentImageResult>()
                .into_iter()
                .filter(|event| event.id == "first" || event.id == "second")
                .map(|event| event.result.as_ref().copied().unwrap())
                .collect::<Vec<_>>();
            if results.len() == 2 {
                break results;
            }
            assert!(Instant::now() < deadline, "agent image load timed out");
            std::thread::yield_now();
        };

        assert_eq!(entities[0], entities[1]);
        assert_eq!(app.world().entity_count(), 1);
        let state = app.world().get_resource::<AgentImageLoaderState>().unwrap();
        assert!(state.pending.is_empty());
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn successful_reload_reuses_the_entity_and_replaces_components() {
        let library = unique_directory("reload-success");
        write_image(&library, "Old soul.\n");
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder").unwrap();
        let entity = load(&mut app, "initial", &reference).unwrap();
        fs::write(image_root(&library).join("SOUL.md"), "New soul.\n").unwrap();

        let reloaded = load(&mut app, "reload", &reference).unwrap();

        assert_eq!(reloaded, entity);
        assert_eq!(app.world().entity_count(), 1);
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn failed_reload_preserves_the_previous_entity() {
        let library = unique_directory("reload-failure");
        write_image(&library, "Stable soul.\n");
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder").unwrap();
        let _entity = load(&mut app, "initial", &reference).unwrap();
        fs::write(image_root(&library).join("unknown.txt"), "invalid").unwrap();

        let error = load(&mut app, "broken", &reference).unwrap_err();

        assert_eq!(error.kind(), AgentImageLoadErrorKind::InvalidLayout);
        assert_eq!(app.world().entity_count(), 1);
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn missing_images_return_not_found_without_creating_entities() {
        let library = unique_directory("missing");
        fs::create_dir_all(&library).unwrap();
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/missing").unwrap();

        let error = load(&mut app, "missing", &reference).unwrap_err();

        assert_eq!(error.kind(), AgentImageLoadErrorKind::NotFound);
        assert_eq!(app.world().entity_count(), 0);
        let _ = fs::remove_dir_all(library);
    }

    #[cfg(unix)]
    #[test]
    fn image_layout_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let library = unique_directory("symlink");
        write_image(&library, "Symlink test.\n");
        symlink(
            image_root(&library).join("SOUL.md"),
            image_root(&library).join("linked-soul"),
        )
        .unwrap();
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder").unwrap();

        let error = load(&mut app, "symlink", &reference).unwrap_err();

        assert_eq!(error.kind(), AgentImageLoadErrorKind::SymlinkNotAllowed);
        assert_eq!(app.world().entity_count(), 0);
        let _ = fs::remove_dir_all(library);
    }
}
