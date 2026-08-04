use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{AppAsyncExt, AsyncTaskError, WorldAsyncExt};
use core_plugin::{App, Component as MecsComponent, Entity, Event, Plugin, Resource, World};
use futures_util::FutureExt;
use margatroid_types::ResourceName;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillLoadErrorKind {
    InvalidRoot,
    InvalidRequest,
    AgentNotAlive,
    VisibilityMissing,
    SourceRootsMissing,
    NotVisible,
    NotFound,
    InvalidSource,
    SymlinkNotAllowed,
    ReadFailed,
    LimitExceeded,
    InvalidUtf8,
    FrontmatterMissing,
    FrontmatterDecodeFailed,
    NameMismatch,
    DescriptionInvalid,
    InstructionsInvalid,
    SourceChanged,
    TaskPanicked,
}

#[derive(Debug)]
pub struct SkillLoadError {
    kind: SkillLoadErrorKind,
    message: String,
}

impl SkillLoadError {
    fn new(kind: SkillLoadErrorKind, message: impl Into<String>) -> Self {
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
        Self::new(SkillLoadErrorKind::InvalidRoot, message)
    }

    pub fn kind(&self) -> SkillLoadErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SkillLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for SkillLoadError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSource {
    Project,
    Image,
    Home,
}

pub struct SkillVisibility {
    names: BTreeSet<ResourceName>,
}

impl SkillVisibility {
    pub fn new() -> Self {
        Self {
            names: BTreeSet::new(),
        }
    }

    pub fn with(mut self, names: impl IntoIterator<Item = ResourceName>) -> Self {
        self.names.extend(names);
        self
    }

    pub fn without(mut self, names: impl IntoIterator<Item = ResourceName>) -> Self {
        for name in names {
            self.names.remove(&name);
        }
        self
    }

    pub fn contains(&self, name: &ResourceName) -> bool {
        self.names.contains(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &ResourceName> + '_ {
        self.names.iter()
    }
}

impl Default for SkillVisibility {
    fn default() -> Self {
        Self::new()
    }
}

impl MecsComponent for SkillVisibility {}

pub struct SkillSourceRoots {
    project_root: Arc<PathBuf>,
    image_root: Arc<PathBuf>,
}

impl SkillSourceRoots {
    pub fn new(project_root: PathBuf, image_root: PathBuf) -> Result<Self, SkillLoadError> {
        Ok(Self {
            project_root: Arc::new(normalize_root(project_root)?),
            image_root: Arc::new(normalize_root(image_root)?),
        })
    }
}

impl MecsComponent for SkillSourceRoots {}

pub struct LoadSkill {
    pub id: String,
    pub agent: Entity,
    pub name: ResourceName,
}

impl Event for LoadSkill {}

pub struct LoadSkillResult {
    pub id: String,
    pub agent: Entity,
    pub name: ResourceName,
    pub result: Result<LoadedSkill, SkillLoadError>,
}

impl Event for LoadSkillResult {}

pub struct LoadedSkill {
    name: ResourceName,
    source: SkillSource,
    root: Arc<PathBuf>,
    description: Arc<str>,
    instructions: Arc<str>,
}

impl LoadedSkill {
    pub fn name(&self) -> &ResourceName {
        &self.name
    }

    pub fn source(&self) -> SkillSource {
        self.source
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    pub fn resolve(
        &self,
        relative: PathBuf,
    ) -> impl Future<Output = Result<PathBuf, SkillLoadError>> + Send + 'static {
        let root = Arc::clone(&self.root);
        async move { resolve_skill_path((*root).clone(), relative).await }
    }
}

pub struct SkillLoaderPlugin {
    home_root: PathBuf,
    schedule: String,
    limits: SkillLoaderLimits,
}

impl SkillLoaderPlugin {
    pub fn open(home_root: impl Into<PathBuf>) -> Result<Self, SkillLoadError> {
        let home_root = normalize_root(home_root.into())?;
        ensure_root(&home_root)?;
        Ok(Self {
            home_root,
            schedule: RuntimePlugin::PRE_UPDATE.to_owned(),
            limits: SkillLoaderLimits::default(),
        })
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Plugin for SkillLoaderPlugin {
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
        if app.world().contains_resource::<SkillLoaderState>() {
            panic!("SkillLoaderPlugin is already installed");
        }
        let schedule = self.schedule;
        let state = SkillLoaderState {
            home_root: Arc::new(self.home_root),
            limits: self.limits,
        };
        app.world_mut().insert_resource(state);
        app.add_system(&schedule, prepare_skill_load_system)
            .add_async_system(&schedule, read_skill)
            .add_system(&schedule, publish_skill_load_system);
    }
}

pub(crate) struct SkillLoaderState {
    home_root: Arc<PathBuf>,
    limits: SkillLoaderLimits,
}

impl Resource for SkillLoaderState {}

#[derive(Clone, Copy)]
struct SkillLoaderLimits {
    max_skill_bytes: u64,
    max_frontmatter_bytes: usize,
    max_description_bytes: usize,
}

impl Default for SkillLoaderLimits {
    fn default() -> Self {
        Self {
            max_skill_bytes: 1024 * 1024,
            max_frontmatter_bytes: 64 * 1024,
            max_description_bytes: 8 * 1024,
        }
    }
}

struct SkillReadTask {
    id: String,
    agent: Entity,
    name: ResourceName,
    project_root: Arc<PathBuf>,
    image_root: Arc<PathBuf>,
    home_root: Arc<PathBuf>,
    limits: SkillLoaderLimits,
}

impl Event for SkillReadTask {}

struct SkillReadOutput {
    payload: Mutex<Option<SkillReadPayload>>,
}

impl SkillReadOutput {
    fn new(payload: SkillReadPayload) -> Self {
        Self {
            payload: Mutex::new(Some(payload)),
        }
    }

    fn take(&self) -> Option<SkillReadPayload> {
        self.payload
            .lock()
            .expect("skill read output lock poisoned")
            .take()
    }
}

struct SkillReadPayload {
    id: String,
    agent: Entity,
    name: ResourceName,
    result: Result<LoadedSkill, SkillLoadError>,
}

struct SkillTaskError {
    source: AsyncTaskError,
}

impl From<AsyncTaskError> for SkillTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}

fn prepare_skill_load_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadSkill>()
        .into_iter()
        .map(|request| (request.id.clone(), request.agent, request.name.clone()))
        .collect::<Vec<_>>();

    for (id, agent, name) in requests {
        if id.is_empty() {
            world.send_event(LoadSkillResult {
                id,
                agent,
                name,
                result: Err(SkillLoadError::new(
                    SkillLoadErrorKind::InvalidRequest,
                    "request id cannot be empty",
                )),
            });
            continue;
        }
        if !world.is_alive(agent) {
            world.send_event(LoadSkillResult {
                id,
                agent,
                name,
                result: Err(SkillLoadError::new(
                    SkillLoadErrorKind::AgentNotAlive,
                    "agent entity is not alive",
                )),
            });
            continue;
        }
        let Some(visibility) = world.get_component::<SkillVisibility>(agent) else {
            world.send_event(LoadSkillResult {
                id,
                agent,
                name,
                result: Err(SkillLoadError::new(
                    SkillLoadErrorKind::VisibilityMissing,
                    "agent does not have SkillVisibility",
                )),
            });
            continue;
        };
        if !visibility.contains(&name) {
            world.send_event(LoadSkillResult {
                id,
                agent,
                name,
                result: Err(SkillLoadError::new(
                    SkillLoadErrorKind::NotVisible,
                    "skill is not visible to the agent",
                )),
            });
            continue;
        }
        let Some(roots) = world.get_component::<SkillSourceRoots>(agent) else {
            world.send_event(LoadSkillResult {
                id,
                agent,
                name,
                result: Err(SkillLoadError::new(
                    SkillLoadErrorKind::SourceRootsMissing,
                    "agent does not have SkillSourceRoots",
                )),
            });
            continue;
        };
        let Some(state) = world.get_resource::<SkillLoaderState>() else {
            panic!("SkillLoaderPlugin is not installed");
        };
        let task = SkillReadTask {
            id,
            agent,
            name,
            project_root: Arc::clone(&roots.project_root),
            image_root: Arc::clone(&roots.image_root),
            home_root: Arc::clone(&state.home_root),
            limits: state.limits,
        };
        world.send_async_event(task);
    }
}

async fn read_skill(task: SkillReadTask) -> Result<SkillReadOutput, SkillTaskError> {
    let route = (task.id.clone(), task.agent, task.name.clone());
    let result = std::panic::AssertUnwindSafe(read_skill_inner(task))
        .catch_unwind()
        .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => Err(SkillLoadError::new(
            SkillLoadErrorKind::TaskPanicked,
            "skill loader task panicked",
        )),
    };
    Ok(SkillReadOutput::new(SkillReadPayload {
        id: route.0,
        agent: route.1,
        name: route.2,
        result,
    }))
}

async fn read_skill_inner(task: SkillReadTask) -> Result<LoadedSkill, SkillLoadError> {
    let resolved = resolve_skill(
        &task.name,
        &task.project_root,
        &task.image_root,
        &task.home_root,
    )
    .await?;
    let path = resolved.root.join("SKILL.md");
    let (source, before) = read_bounded(&path, task.limits.max_skill_bytes).await?;
    let (description, instructions) = parse_skill_markdown(&task.name, &source, &task.limits)?;
    let after = file_signature(&path).await?;
    if before != after {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::SourceChanged,
            "SKILL.md changed while it was being read",
        ));
    }
    Ok(LoadedSkill {
        name: task.name,
        source: resolved.source,
        root: Arc::new(resolved.root),
        description: Arc::from(description),
        instructions: Arc::from(instructions),
    })
}

fn publish_skill_load_system(world: &mut World) {
    let mut results = Vec::new();
    for output in world.event_reader::<Result<SkillReadOutput, SkillTaskError>>() {
        match output {
            Ok(output) => results.extend(output.take()),
            Err(error) => {
                tracing::error!(error = %error.source, "skill loader async task stopped");
            }
        }
    }
    for output in results {
        world.send_event(LoadSkillResult {
            id: output.id,
            agent: output.agent,
            name: output.name,
            result: output.result,
        });
    }
}

struct ResolvedSkill {
    source: SkillSource,
    root: PathBuf,
}

async fn resolve_skill(
    name: &ResourceName,
    project_root: &Path,
    image_root: &Path,
    home_root: &Path,
) -> Result<ResolvedSkill, SkillLoadError> {
    for (source, root) in [
        (SkillSource::Project, project_root),
        (SkillSource::Image, image_root),
        (SkillSource::Home, home_root),
    ] {
        if let Some(root) = check_candidate(root, name).await? {
            return Ok(ResolvedSkill { source, root });
        }
    }
    Err(SkillLoadError::new(
        SkillLoadErrorKind::NotFound,
        format!("skill `{name}` was not found"),
    ))
}

#[derive(Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
}

fn parse_skill_markdown(
    name: &ResourceName,
    source: &str,
    limits: &SkillLoaderLimits,
) -> Result<(String, String), SkillLoadError> {
    let first_end = source.find('\n').ok_or_else(|| {
        SkillLoadError::new(
            SkillLoadErrorKind::FrontmatterMissing,
            "frontmatter is missing",
        )
    })?;
    if source[..first_end].trim_end_matches('\r') != "---" {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::FrontmatterMissing,
            "SKILL.md must start with a frontmatter delimiter",
        ));
    }
    let rest = &source[first_end + 1..];
    let mut offset = 0;
    let mut close = None;
    for line in rest.split_inclusive('\n') {
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        if content == "---" {
            close = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let Some((frontmatter_end, instructions_start)) = close else {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::FrontmatterMissing,
            "frontmatter closing delimiter is missing",
        ));
    };
    if frontmatter_end > limits.max_frontmatter_bytes {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::LimitExceeded,
            "frontmatter exceeds the configured limit",
        ));
    }
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(&rest[..frontmatter_end]).map_err(|error| {
            SkillLoadError::new(
                SkillLoadErrorKind::FrontmatterDecodeFailed,
                format!("invalid frontmatter: {error}"),
            )
        })?;
    if frontmatter.name != name.name() {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::NameMismatch,
            "frontmatter name does not match the directory name",
        ));
    }
    let description = frontmatter.description.trim().to_owned();
    if description.is_empty() || description.len() > limits.max_description_bytes {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::DescriptionInvalid,
            "skill description is empty or too large",
        ));
    }
    let instructions = rest[instructions_start..].to_owned();
    if instructions.trim().is_empty() {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::InstructionsInvalid,
            "skill instructions cannot be empty",
        ));
    }
    Ok((description, instructions))
}

async fn resolve_skill_path(root: PathBuf, relative: PathBuf) -> Result<PathBuf, SkillLoadError> {
    if relative.as_os_str().is_empty() || relative.is_absolute() || has_parent(&relative) {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::InvalidSource,
            "skill path must be a non-empty relative path without parent traversal",
        ));
    }
    let path = root.join(relative);
    ensure_existing_path(&root, &path).await
}

fn normalize_root(root: PathBuf) -> Result<PathBuf, SkillLoadError> {
    if !root.is_absolute() || has_parent(&root) {
        return Err(SkillLoadError::invalid_root(
            "skill root must be an absolute path without parent traversal",
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

fn ensure_root(root: &Path) -> Result<(), SkillLoadError> {
    std::fs::create_dir_all(root).map_err(|error| {
        SkillLoadError::invalid_root(format!("cannot create skill root: {error}"))
    })?;
    let metadata = std::fs::symlink_metadata(root).map_err(|error| {
        SkillLoadError::invalid_root(format!("cannot inspect skill root: {error}"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SkillLoadError::invalid_root(
            "skill root must be a directory and cannot be a symlink",
        ));
    }
    Ok(())
}

async fn check_candidate(
    root: &Path,
    name: &ResourceName,
) -> Result<Option<PathBuf>, SkillLoadError> {
    let Some(root) = check_directory(root, true).await? else {
        return Ok(None);
    };
    let scope = root.join(name.scope());
    let Some(scope) = check_directory(&scope, false).await? else {
        return Ok(None);
    };
    let skill = scope.join(name.name());
    let Some(skill) = check_directory(&skill, false).await? else {
        return Ok(None);
    };
    Ok(Some(skill))
}

async fn check_directory(path: &Path, root: bool) -> Result<Option<PathBuf>, SkillLoadError> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SkillLoadError::new(
                SkillLoadErrorKind::ReadFailed,
                format!("cannot inspect skill directory: {error}"),
            ))
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::SymlinkNotAllowed,
            "skill directory cannot contain symlinks",
        ));
    }
    if !metadata.is_dir() {
        return Err(SkillLoadError::new(
            if root {
                SkillLoadErrorKind::InvalidRoot
            } else {
                SkillLoadErrorKind::InvalidSource
            },
            "skill source must be a directory",
        ));
    }
    Ok(Some(path.to_path_buf()))
}

async fn ensure_existing_path(root: &Path, path: &Path) -> Result<PathBuf, SkillLoadError> {
    if !path.starts_with(root) {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::InvalidSource,
            "skill path escapes its source root",
        ));
    }
    let root_metadata = tokio::fs::symlink_metadata(root).await.map_err(|error| {
        SkillLoadError::new(
            SkillLoadErrorKind::ReadFailed,
            format!("cannot inspect skill source root: {error}"),
        )
    })?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::SymlinkNotAllowed,
            "skill source root was replaced by a symlink or invalid file",
        ));
    }
    let mut current = root.to_path_buf();
    for component in path.strip_prefix(root).unwrap().components() {
        let Component::Normal(part) = component else {
            return Err(SkillLoadError::new(
                SkillLoadErrorKind::InvalidSource,
                "skill path contains an invalid component",
            ));
        };
        current.push(part);
        let metadata = tokio::fs::symlink_metadata(&current)
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    SkillLoadError::new(SkillLoadErrorKind::NotFound, "skill path does not exist")
                } else {
                    SkillLoadError::new(SkillLoadErrorKind::ReadFailed, error.to_string())
                }
            })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() && !metadata.is_dir() {
            return Err(SkillLoadError::new(
                SkillLoadErrorKind::SymlinkNotAllowed,
                "skill path contains a symlink or special file",
            ));
        }
    }
    Ok(current)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileSignature {
    length: u64,
    modified: Option<SystemTime>,
}

async fn read_bounded(
    path: &Path,
    maximum: u64,
) -> Result<(String, FileSignature), SkillLoadError> {
    let before = file_signature(path).await?;
    if before.length > maximum {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::LimitExceeded,
            "SKILL.md exceeds the configured limit",
        ));
    }
    let bytes = tokio::fs::read(path).await.map_err(|error| {
        SkillLoadError::new(
            SkillLoadErrorKind::ReadFailed,
            format!("cannot read SKILL.md: {error}"),
        )
    })?;
    let after = file_signature(path).await?;
    if before != after || bytes.len() as u64 != before.length {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::SourceChanged,
            "SKILL.md changed while it was being read",
        ));
    }
    let source = String::from_utf8(bytes).map_err(|_| {
        SkillLoadError::new(
            SkillLoadErrorKind::InvalidUtf8,
            "SKILL.md is not valid UTF-8",
        )
    })?;
    Ok((source, before))
}

async fn file_signature(path: &Path) -> Result<FileSignature, SkillLoadError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        SkillLoadError::new(
            SkillLoadErrorKind::ReadFailed,
            format!("cannot inspect SKILL.md: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(SkillLoadError::new(
            SkillLoadErrorKind::SymlinkNotAllowed,
            "SKILL.md must be a regular file and cannot be a symlink",
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
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use core_plugin::App;

    use super::*;

    fn write_skill(root: &Path, name: &str, description: &str, instructions: &str) {
        let path = root.join("local").join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{instructions}\n"),
        )
        .unwrap();
    }

    fn unique_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "margatroid-skill-loader-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn loads_the_highest_priority_skill_without_blocking_the_frame() {
        let directory = unique_directory("priority");
        fs::create_dir_all(&directory).unwrap();
        let home = directory.join("home");
        let project = directory.join("project");
        let image = directory.join("image");
        write_skill(&home, "review", "home", "home instructions");
        write_skill(&image, "review", "image", "image instructions");
        write_skill(&project, "review", "project", "project instructions");

        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(SkillLoaderPlugin::open(home).unwrap());
        let agent = app.world_mut().spawn();
        let name = ResourceName::new("local/review").unwrap();
        app.world_mut()
            .insert_component(agent, SkillVisibility::new().with([name.clone()]));
        app.world_mut()
            .insert_component(agent, SkillSourceRoots::new(project, image).unwrap());
        app.world().send_event(LoadSkill {
            id: "load-1".into(),
            agent,
            name,
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(result) = app
                .world()
                .event_reader::<LoadSkillResult>()
                .into_iter()
                .next()
            {
                let loaded = result.result.as_ref().unwrap();
                assert_eq!(loaded.source(), SkillSource::Project);
                assert_eq!(loaded.description(), "project");
                assert_eq!(loaded.instructions(), "project instructions\n");
                break;
            }
            assert!(Instant::now() < deadline, "skill load timed out");
            std::thread::yield_now();
        }
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn disabled_skills_are_not_visible() {
        let name = ResourceName::new("local/review").unwrap();
        let visibility = SkillVisibility::new()
            .with([name.clone()])
            .without([name.clone()]);

        assert!(!visibility.contains(&name));
        assert_eq!(visibility.names().count(), 0);
    }

    #[test]
    fn invalid_project_skill_does_not_fall_back_to_the_image() {
        let directory = unique_directory("invalid-project");
        let home = directory.join("home");
        let project = directory.join("project");
        let image = directory.join("image");
        write_skill(&image, "review", "image", "image instructions");
        write_skill(&project, "wrong-name", "project", "project instructions");
        let project_skill = project.join("local/review");
        fs::create_dir_all(&project_skill).unwrap();
        fs::rename(
            project.join("local/wrong-name/SKILL.md"),
            project_skill.join("SKILL.md"),
        )
        .unwrap();

        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(SkillLoaderPlugin::open(home).unwrap());
        let agent = app.world_mut().spawn();
        let name = ResourceName::new("local/review").unwrap();
        app.world_mut()
            .insert_component(agent, SkillVisibility::new().with([name.clone()]));
        app.world_mut()
            .insert_component(agent, SkillSourceRoots::new(project, image).unwrap());
        app.world().send_event(LoadSkill {
            id: "load-invalid".into(),
            agent,
            name,
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(result) = app
                .world()
                .event_reader::<LoadSkillResult>()
                .into_iter()
                .next()
            {
                let Err(error) = &result.result else {
                    panic!("invalid project skill must fail");
                };
                assert_eq!(error.kind(), SkillLoadErrorKind::NameMismatch);
                break;
            }
            assert!(Instant::now() < deadline, "skill load timed out");
            std::thread::yield_now();
        }
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn markdown_parser_preserves_leading_instruction_lines() {
        let name = ResourceName::new("local/review").unwrap();
        let source = "---\nname: review\ndescription: Review code\n---\n\nFirst step\n";

        let (_, instructions) =
            parse_skill_markdown(&name, source, &SkillLoaderLimits::default()).unwrap();

        assert_eq!(instructions, "\nFirst step\n");
    }

    #[test]
    fn error_messages_are_truncated_on_utf8_boundaries() {
        let error = SkillLoadError::new(SkillLoadErrorKind::ReadFailed, "界".repeat(300));

        assert!(error.message().len() <= 512);
        assert!(error.message().ends_with("..."));
        assert!(error.message().is_char_boundary(error.message().len()));
    }

    #[tokio::test]
    async fn auxiliary_paths_cannot_escape_the_skill_root() {
        let root = unique_directory("resolve");
        fs::create_dir_all(&root).unwrap();

        let error = resolve_skill_path(root.clone(), PathBuf::from("../secret"))
            .await
            .unwrap_err();

        assert_eq!(error.kind(), SkillLoadErrorKind::InvalidSource);
        let _ = fs::remove_dir_all(root);
    }
}
