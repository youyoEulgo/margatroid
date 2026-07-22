use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

const WORKSPACE_DIR_NAME: &str = "workspace";
const APP_CONFIG_NAME: &str = "margatroid.toml";
const WORKSPACE_CONFIG_NAME: &str = "workspace.toml";
const WORKSPACE_DATA_DIR_NAME: &str = "data";

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
    // #[cfg(target_os = "windows")]
    // {
    //     dirs::data_local_dir().map(|p| p.join("Margatroid"))
    // }

    #[cfg(not(target_os = "windows"))]
    {
        dirs::home_dir().map(|p| p.join(".margatroid"))
    }
}
