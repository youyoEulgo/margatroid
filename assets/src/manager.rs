use anyhow::{Context, Result, bail};
use paths::MargatroidPaths;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use types::{AppConfig, ComposeFile, WorkspaceConfig};

/// Margatroid 资源管理器
///
/// 统一管理全局配置与 Workspace 生命周期：
/// - app config（margatroid.toml）的读写与内存缓存
/// - workspace 的创建（从 compose 文件）、列表、销毁
/// - workspace.toml 的读写与内存缓存
///
/// # 用法
///
/// ```ignore
/// let mut mgr = assets::Manager::new(paths).init()?;
/// mgr.create_workspace(&compose)?;
/// for name in mgr.list_workspaces() {
///     println!("{}", name);
/// }
/// ```
#[derive(Debug)]
pub struct Manager {
    paths: Arc<MargatroidPaths>,
    app_config: AppConfig,
    workspace_configs: HashMap<String, WorkspaceConfig>,
}

// ── Lifecycle ─────────────────────────────────────────────────

impl Manager {
    pub fn new(paths: Arc<MargatroidPaths>) -> Self {
        Self {
            paths,
            app_config: AppConfig::default(),
            workspace_configs: HashMap::new(),
        }
    }

    /// 初始化全局配置并加载所有已有 workspace 到缓存
    ///
    /// 消费 `self` 并返回已初始化的 `Manager`，支持链式调用。
    pub fn init(mut self) -> Result<Self> {
        self.init_app()?;
        self.init_workspaces()?;
        Ok(self)
    }

    /// 使用默认路径初始化
    ///
    /// 等价于 `Manager::new(paths).init()`，但自动确定 `~/.margatroid/` 路径。
    pub fn bootstrap() -> Result<Self> {
        let root = paths::margatroid_root().unwrap_or_else(|| PathBuf::from(".margatroid"));
        Manager::new(Arc::new(MargatroidPaths::new(root))).init()
    }

    pub fn paths(&self) -> &Arc<MargatroidPaths> {
        &self.paths
    }
}

// ── App Config ───────────────────────────────────────────────

impl Manager {
    pub fn app_config(&self) -> &AppConfig {
        &self.app_config
    }

    pub fn save_app_config(&self) -> Result<()> {
        self.write_app_config(&self.app_config)
    }

    fn init_app(&mut self) -> Result<()> {
        match self.load_app_config() {
            Ok(_) => Ok(()),
            Err(e) => {
                let is_not_found = e
                    .root_cause()
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                if is_not_found {
                    let config = AppConfig::default();
                    self.write_app_config(&config)?;
                    self.load_app_config()?;
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    fn write_app_config(&self, config: &AppConfig) -> Result<()> {
        let config_path = self.paths.app_config();
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
        write_atomic(config_path, config)
    }

    fn read_app_config(&self) -> Result<AppConfig> {
        let config_path = self.paths.app_config();
        let content = fs::read_to_string(config_path)
            .with_context(|| format!("Failed to read configuration: {}", config_path.display()))?;
        let config: AppConfig = toml::from_str(&content)
            .with_context(|| format!("TOML invalid in: {}", config_path.display()))?;
        Ok(config)
    }

    fn load_app_config(&mut self) -> Result<()> {
        self.app_config = self.read_app_config()?;
        Ok(())
    }
}

// ── Workspace ────────────────────────────────────────────────

impl Manager {
    /// 从 compose 文件创建 Workspace
    ///
    /// 创建目录结构，写入默认 workspace.toml，为每个 agent 创建 data/ 子目录，
    /// 并将新 workspace 加载到内存缓存中。
    pub fn create_workspace(&mut self, compose: &ComposeFile) -> Result<()> {
        let name = &compose.workspace.name;
        paths::validate_segment(name)?;

        // 1. 目录 + workspace.toml
        let ws_dir = self.paths.workspace_dir(name)?;
        if !ws_dir.is_dir() {
            fs::create_dir_all(&ws_dir)
                .with_context(|| format!("创建 workspace 目录失败: {}", ws_dir.display()))?;
        }

        let config = WorkspaceConfig::default();
        self.write_workspace_config(name, &config)?;
        self.workspace_configs.insert(name.into(), config);

        // 2. 写入默认沙箱配置
        let sandbox_path = self.paths.workspace_dir(name)?.join("sandbox.toml");
        fs::write(&sandbox_path, DEFAULT_SANDBOX_CONFIG)?;

        // 3. agent data 子目录
        for agent in &compose.agents {
            let data_dir = self.paths.workspace_data_dir(name)?.join(&agent.id);
            fs::create_dir_all(&data_dir)
                .with_context(|| format!("创建 agent {} 的数据目录失败", agent.id))?;
        }

        Ok(())
    }

    /// 从 compose 文件路径创建 Workspace
    pub fn create_workspace_from_file(&mut self, compose_path: impl AsRef<Path>) -> Result<()> {
        let compose = compose::load(compose_path)?;
        self.create_workspace(&compose)
    }

    /// 列出所有 Workspace（从内存缓存，非文件系统扫描）
    pub fn list_workspaces(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.workspace_configs.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// 获取指定 workspace 的配置
    pub fn workspace_config(&self, name: &str) -> Option<&WorkspaceConfig> {
        self.workspace_configs.get(name)
    }

    /// 销毁一个 Workspace（删除目录树 + 清除缓存）
    pub fn destroy_workspace(&mut self, name: &str) -> Result<()> {
        let ws_dir = self.paths.workspace_dir(name)?;
        if ws_dir.is_dir() {
            fs::remove_dir_all(&ws_dir)
                .with_context(|| format!("删除 workspace 目录失败: {}", ws_dir.display()))?;
        }
        self.workspace_configs.remove(name);
        Ok(())
    }

    /// 持久化指定 workspace 的配置
    pub fn save_workspace_config(&self, name: &str) -> Result<()> {
        match self.workspace_configs.get(name) {
            Some(config) => self.write_workspace_config(name, config),
            None => bail!("workspace '{}' 不在缓存中", name),
        }
    }

    // ── private workspace helpers ──

    fn init_workspaces(&mut self) -> Result<()> {
        let names = self.scan_workspace_dirs()?;
        for name in names {
            let config = self.read_workspace_config(&name)?;
            self.workspace_configs.insert(name, config);
        }
        Ok(())
    }

    fn write_workspace_config(&self, name: &str, config: &WorkspaceConfig) -> Result<()> {
        let config_path = self.paths.workspace_config(name)?;
        write_atomic(&config_path, config)
    }

    fn read_workspace_config(&self, name: &str) -> Result<WorkspaceConfig> {
        let config_path = self.paths.workspace_config(name)?;
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read configuration: {}", config_path.display()))?;
        let config: WorkspaceConfig = toml::from_str(&content)
            .with_context(|| format!("TOML invalid in: {}", config_path.display()))?;
        Ok(config)
    }

    fn scan_workspace_dirs(&self) -> Result<Vec<String>> {
        let base = self.paths.workspaces_base();
        if !base.is_dir() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(base)
            .with_context(|| format!("读取 workspace 目录失败: {}", base.display()))?
        {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("Warning: Failed to read directory entry: {}", e);
                    continue;
                }
            };
            if !entry.file_type().map_or(false, |t| t.is_dir()) {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                if self
                    .paths
                    .workspace_config(name)
                    .ok()
                    .is_some_and(|p| p.is_file())
                {
                    names.push(name.to_string());
                }
            }
        }
        Ok(names)
    }
}

/// 默认沙箱配置模板（workspace 创建时生成）
const DEFAULT_SANDBOX_CONFIG: &str = r#"# Margatroid Sandbox Configuration

enabled = true
auto_allow_bash_if_sandboxed = false
allow_unsandboxed_commands = false
excluded_commands = ["git push", "gh pr create"]

[filesystem]
deny_read = []
allow_write = []
deny_write = []

[network]
allowed_domains = ["github.com", "*.github.com", "api.github.com", "registry.npmjs.org"]
denied_domains = []
allow_unix_sockets = []
allow_local_binding = false
"#;

// ── Helpers ──────────────────────────────────────────────────

/// 原子写入 TOML 文件（先写 .tmp 再 rename）
fn write_atomic(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut temp_path = path.as_os_str().to_os_string();
    temp_path.push(".tmp");
    let temp_path = PathBuf::from(temp_path);

    let content = toml::to_string_pretty(value).context("TOML 序列化失败")?;
    fs::write(&temp_path, &content)
        .with_context(|| format!("写入临时文件失败: {}", temp_path.display()))?;
    fs::rename(&temp_path, path)
        .with_context(|| format!("rename 失败: {} -> {}", temp_path.display(), path.display()))?;

    Ok(())
}
