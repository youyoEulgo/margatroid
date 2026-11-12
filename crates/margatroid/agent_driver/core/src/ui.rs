use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SETTLE: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug)]
pub struct DriverUiReadLimits {
    pub raw_bytes: usize,
}
impl Default for DriverUiReadLimits {
    fn default() -> Self {
        Self {
            raw_bytes: 32 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverUiFile {
    pub path: String,
    pub media_type: String,
    pub data: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverUiBundle {
    pub revision: String,
    pub entry: String,
    pub files: Vec<DriverUiFile>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverUiError {
    Initializing,
    EntryMissing,
    ReadFailed(String),
    PathInvalid,
    TooLarge,
    Unstable,
}
impl fmt::Display for DriverUiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initializing => f.write_str("Driver UI is not ready"),
            Self::EntryMissing => f.write_str("Driver UI entry is missing"),
            Self::ReadFailed(value) => write!(f, "Driver UI read failed: {value}"),
            Self::PathInvalid => f.write_str("Driver UI path is invalid"),
            Self::TooLarge => f.write_str("Driver UI exceeds the resource budget"),
            Self::Unstable => f.write_str("Driver UI files changed while reading"),
        }
    }
}
impl std::error::Error for DriverUiError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverUiState {
    Initializing,
    Ready(Option<Arc<DriverUiBundle>>),
    Failed(DriverUiError),
}
#[derive(Clone)]
pub struct DriverUiWatch {
    shared: Arc<DriverUiShared>,
}
struct DriverUiShared {
    state: RwLock<DriverUiState>,
    cancel: CancellationToken,
    on_change: Arc<dyn Fn(DriverUiState) + Send + Sync>,
}
impl DriverUiWatch {
    pub fn state(&self) -> DriverUiState {
        self.shared
            .state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn cancel(&self) {
        self.shared.cancel.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.is_cancelled()
    }
    pub fn publish(&self, value: DriverUiState) {
        if self.shared.cancel.is_cancelled() {
            return;
        }
        {
            let mut previous = self.shared.state.write().unwrap_or_else(|e| e.into_inner());
            if *previous == value {
                return;
            }
            *previous = value.clone();
        }
        if !self.shared.cancel.is_cancelled() {
            (self.shared.on_change)(value);
        }
    }
}
pub struct DriverUiMonitor {
    manifest_path: PathBuf,
    limits: DriverUiReadLimits,
    watch: DriverUiWatch,
}
pub fn open_driver_ui_watch(
    manifest_path: PathBuf,
    on_change: Arc<dyn Fn(DriverUiState) + Send + Sync>,
) -> (DriverUiWatch, DriverUiMonitor) {
    let watch = DriverUiWatch {
        shared: Arc::new(DriverUiShared {
            state: RwLock::new(DriverUiState::Initializing),
            cancel: CancellationToken::new(),
            on_change,
        }),
    };
    let monitor = DriverUiMonitor {
        manifest_path,
        limits: DriverUiReadLimits::default(),
        watch: watch.clone(),
    };
    (watch, monitor)
}
fn read_error(error: io::Error) -> DriverUiError {
    if error.kind() == io::ErrorKind::NotFound {
        DriverUiError::Unstable
    } else {
        DriverUiError::ReadFailed(error.to_string())
    }
}
fn account(used: &mut usize, size: usize, maximum: usize) -> Result<(), DriverUiError> {
    *used = used.checked_add(size).ok_or(DriverUiError::TooLarge)?;
    if *used > maximum {
        return Err(DriverUiError::TooLarge);
    }
    Ok(())
}
fn scan(
    root: &Path,
    directory: &Path,
    relative: &Path,
    visited: &mut BTreeSet<PathBuf>,
    files: &mut Vec<DriverUiFile>,
    used: &mut usize,
    limits: DriverUiReadLimits,
) -> Result<(), DriverUiError> {
    let canonical = directory.canonicalize().map_err(read_error)?;
    if !canonical.starts_with(root) || !visited.insert(canonical.clone()) {
        return Err(DriverUiError::PathInvalid);
    }
    let mut entries = fs::read_dir(&canonical)
        .map_err(read_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(read_error)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let logical = relative.join(entry.file_name());
        let path = entry.path().canonicalize().map_err(read_error)?;
        if !path.starts_with(root) {
            return Err(DriverUiError::PathInvalid);
        }
        let metadata = fs::metadata(&path).map_err(read_error)?;
        if metadata.is_dir() {
            scan(root, &path, &logical, visited, files, used, limits)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(DriverUiError::PathInvalid);
        }
        let path_text = logical
            .to_str()
            .ok_or(DriverUiError::PathInvalid)?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if path_text.contains('\\')
            || path_text.chars().any(char::is_control)
            || path_text
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
        {
            return Err(DriverUiError::PathInvalid);
        }
        let media = mime_guess::from_path(&logical)
            .first_or_octet_stream()
            .to_string();
        let media_type = if media.starts_with("text/")
            || matches!(
                media.as_str(),
                "application/javascript" | "application/json"
            ) {
            format!("{media}; charset=utf-8")
        } else {
            media
        };
        account(used, path_text.len() + media_type.len(), limits.raw_bytes)?;
        let remaining = limits.raw_bytes - *used;
        if metadata.len() > remaining as u64 {
            return Err(DriverUiError::TooLarge);
        }
        let file = File::open(&path).map_err(read_error)?;
        if !file.metadata().map_err(read_error)?.is_file() {
            return Err(DriverUiError::PathInvalid);
        }
        let mut data = Vec::new();
        file.take(remaining as u64 + 1)
            .read_to_end(&mut data)
            .map_err(read_error)?;
        account(used, data.len(), limits.raw_bytes)?;
        if data.len() as u64 != metadata.len()
            || entry.path().canonicalize().map_err(read_error)? != path
        {
            return Err(DriverUiError::Unstable);
        }
        files.push(DriverUiFile {
            path: path_text,
            media_type,
            data,
        });
    }
    Ok(())
}
fn read_once(
    manifest_path: &Path,
    limits: DriverUiReadLimits,
) -> Result<Option<DriverUiBundle>, DriverUiError> {
    let directory = manifest_path.parent().ok_or(DriverUiError::PathInvalid)?;
    let driver = match directory.canonicalize() {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(read_error(error)),
    };
    let root = match driver.join("ui").canonicalize() {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(read_error(error)),
    };
    if root == driver || !root.starts_with(&driver) || !root.is_dir() {
        return Err(DriverUiError::PathInvalid);
    }
    let mut files = Vec::new();
    scan(
        &root,
        &root,
        Path::new(""),
        &mut BTreeSet::new(),
        &mut files,
        &mut 0,
        limits,
    )?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if !files.iter().any(|file| file.path == "index.html") {
        return Err(DriverUiError::EntryMissing);
    }
    let mut digest = Sha256::new();
    for file in &files {
        for value in [
            file.path.as_bytes(),
            file.media_type.as_bytes(),
            file.data.as_slice(),
        ] {
            digest.update((value.len() as u64).to_le_bytes());
            digest.update(value);
        }
    }
    Ok(Some(DriverUiBundle {
        revision: format!("sha256:{:x}", digest.finalize()),
        entry: "index.html".into(),
        files,
    }))
}
pub fn read_driver_ui(
    manifest_path: &Path,
    limits: DriverUiReadLimits,
) -> Result<Option<Arc<DriverUiBundle>>, DriverUiError> {
    let first = read_once(manifest_path, limits)?;
    let second = read_once(manifest_path, limits)?;
    if first != second {
        return Err(DriverUiError::Unstable);
    }
    Ok(second.map(Arc::new))
}
async fn refresh(
    context: &DriverUiMonitor,
    monitor_error: Option<DriverUiError>,
) -> Result<bool, DriverUiError> {
    let path = context.manifest_path.clone();
    let limits = context.limits;
    let watch = context.watch.clone();
    tokio::task::spawn_blocking(move || {
        if watch.shared.cancel.is_cancelled() {
            return Ok(false);
        }
        match read_driver_ui(&path, limits) {
            Err(DriverUiError::Unstable) => Ok(true),
            Ok(value) => {
                let state = match (value, monitor_error) {
                    (Some(_), Some(error)) => DriverUiState::Failed(error),
                    (value, _) => DriverUiState::Ready(value),
                };
                watch.publish(state);
                Ok(false)
            }
            Err(error) => {
                watch.publish(DriverUiState::Failed(error));
                Ok(false)
            }
        }
    })
    .await
    .map_err(|error| DriverUiError::ReadFailed(error.to_string()))?
}
fn attach(
    manifest: PathBuf,
    sender: mpsc::Sender<()>,
) -> Result<RecommendedWatcher, DriverUiError> {
    let directory = manifest.parent().ok_or(DriverUiError::PathInvalid)?;
    let ui = directory.join("ui");
    let parent = directory.parent().unwrap_or(directory).to_owned();
    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<notify::Event>| {
            let relevant = match event {
                Ok(event) => {
                    !matches!(event.kind, notify::EventKind::Access(_))
                        && event
                            .paths
                            .iter()
                            .any(|path| path.starts_with(&ui) || ui.starts_with(path))
                }
                Err(_) => true,
            };
            if relevant {
                let _ = sender.try_send(());
            }
        },
        Config::default(),
    )
    .map_err(|e| DriverUiError::ReadFailed(e.to_string()))?;
    watcher
        .watch(&parent, RecursiveMode::Recursive)
        .map_err(|e| DriverUiError::ReadFailed(e.to_string()))?;
    Ok(watcher)
}
pub async fn run_driver_ui_monitor(context: DriverUiMonitor) -> Result<(), DriverUiError> {
    let (sender, mut events) = mpsc::channel(1);
    let cancel = context.watch.shared.cancel.clone();
    let mut watcher = None;
    let mut dirty = true;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let mut monitor_error = None;
        if watcher.is_none() {
            let path = context.manifest_path.clone();
            let signal = sender.clone();
            match tokio::task::spawn_blocking(move || attach(path, signal)).await {
                Ok(Ok(value)) => watcher = Some(value),
                Ok(Err(error)) => monitor_error = Some(error),
                Err(error) => return Err(DriverUiError::ReadFailed(error.to_string())),
            }
        }
        if dirty {
            dirty = refresh(&context, monitor_error).await?;
            if cancel.is_cancelled() {
                return Ok(());
            }
        }
        tokio::select! {
            biased;
            _=cancel.cancelled()=>return Ok(()),
            _=events.recv()=>dirty=true,
            _=tokio::time::sleep(SETTLE), if watcher.is_none() || dirty=>dirty=true,
        }
        if dirty {
            loop {
                tokio::select! {
                    biased;
                    _=cancel.cancelled()=>return Ok(()),
                    _=events.recv()=>(),
                    _=tokio::time::sleep(SETTLE)=>break,
                }
            }
            let parent = context.manifest_path.parent().and_then(Path::parent);
            if parent.is_some_and(|path| !path.exists()) {
                watcher = None;
            }
        }
    }
}
