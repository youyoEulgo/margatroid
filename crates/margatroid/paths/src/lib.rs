use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const WORKSPACE_DIR_NAME: &str = "workspace";
const APP_CONFIG_NAME: &str = "margatroid.toml";
const WORKSPACE_CONFIG_NAME: &str = "workspace.toml";
const WORKSPACE_DATA_DIR_NAME: &str = "data";
const DAEMON_LOCK_NAME: &str = "margatroidd.lock";

#[derive(Debug, Clone)]
pub struct MargatroidPaths {
    root: PathBuf,
    workspace_base: PathBuf,
    app_config: PathBuf,
}

impl MargatroidPaths {
    /// 基于给定的 root 目录，锚定全部路径布局
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let workspace_base = root.join(WORKSPACE_DIR_NAME);
        let app_config = root.join(APP_CONFIG_NAME);
        Self {
            root,
            workspace_base,
            app_config,
        }
    }

    /// 用户指定的 Margatroid 根目录
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 全局配置文件：`{root}/margatroid.toml`
    pub fn app_config(&self) -> &Path {
        &self.app_config
    }

    /// 所有 workspace 的父目录：`{root}/workspace/`
    pub fn workspaces_base(&self) -> &Path {
        &self.workspace_base
    }

    /// 单个 workspace 的目录：`{root}/workspace/{name}/`
    pub fn workspace_dir(&self, name: &str) -> Result<PathBuf> {
        validate_segment(name)?;
        Ok(self.workspace_base.join(name))
    }

    /// workspace 的配置文件：`{root}/workspace/{name}/workspace.toml`
    pub fn workspace_config(&self, name: &str) -> Result<PathBuf> {
        Ok(self.workspace_dir(name)?.join(WORKSPACE_CONFIG_NAME))
    }

    /// workspace 的数据目录：`{root}/workspace/{name}/data/`
    pub fn workspace_data_dir(&self, name: &str) -> Result<PathBuf> {
        Ok(self.workspace_dir(name)?.join(WORKSPACE_DATA_DIR_NAME))
    }
}

pub fn validate_segment(s: &str) -> Result<()> {
    if s.is_empty() {
        bail!("InvalidSegment: empty -> {}", s);
    }
    if s == "." || s == ".." {
        bail!("InvalidSegment: relative component -> {}", s);
    }
    if s.contains('/') || s.contains('\\') {
        bail!("InvalidSegment: relative contains path separator -> {}", s);
    }
    Ok(())
}

pub fn margatroid_root() -> Option<PathBuf> {
    dirs::home_dir().map(|path| path.join(".margatroid"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonPaths {
    data_dir: PathBuf,
    config: PathBuf,
    lock: PathBuf,
}

impl DaemonPaths {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            config: data_dir.join(APP_CONFIG_NAME),
            lock: data_dir.join(DAEMON_LOCK_NAME),
            data_dir,
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn config(&self) -> &Path {
        &self.config
    }

    pub fn lock(&self) -> &Path {
        &self.lock
    }

    pub fn prepare(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("cannot create data directory {}", self.data_dir.display()))?;
        set_private_directory_permissions(&self.data_dir)
    }
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("cannot secure data directory {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod daemon_tests {
    use super::*;

    #[test]
    fn daemon_paths_share_one_root() {
        let paths = DaemonPaths::new("/tmp/margatroid-test");
        assert_eq!(
            paths.config(),
            Path::new("/tmp/margatroid-test/margatroid.toml")
        );
        assert_eq!(
            paths.lock(),
            Path::new("/tmp/margatroid-test/margatroidd.lock")
        );
    }
}
