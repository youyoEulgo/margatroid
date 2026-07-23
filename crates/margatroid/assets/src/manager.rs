use anyhow::{Context, Result, bail};
use paths::MargatroidPaths;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use types::{AppConfig, WorkspaceConfig};

/// Margatroid 资源管理器
///
/// 统一管理全局配置与 Workspace 生命周期：
/// - app config（margatroid.toml）的读写与内存缓存
/// - workspace 的列表、销毁和旧配置读取
/// - workspace.toml 的读写与内存缓存
///
/// # 用法
///
/// ```ignore
/// let mut mgr = assets::Manager::new(paths).init()?;
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

    /// 确保 workspace 有系统提示词文件，若无则创建默认文件
    /// 返回提示词内容
    pub fn ensure_system_prompt(&self, name: &str) -> Result<String> {
        let path = self.paths.workspace_dir(name)?.join("system_prompt.md");
        if path.exists() {
            return fs::read_to_string(&path)
                .with_context(|| format!("读取系统提示词失败: {}", path.display()));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败: {}", parent.display()))?;
        }
        fs::write(&path, DEFAULT_SYSTEM_PROMPT)?;
        Ok(DEFAULT_SYSTEM_PROMPT.to_string())
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
            if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                continue;
            }
            if let Some(name) = entry.file_name().to_str()
                && self
                    .paths
                    .workspace_config(name)
                    .ok()
                    .is_some_and(|p| p.is_file())
            {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }
}

const DEFAULT_SYSTEM_PROMPT: &str = r#"# 系统提示词

这是一个多智能体工作区。你是团队中的一员，与其他成员协作完成任务。

## 基本规则（必须遵守）
- 每次回答至少调用一个工具（delegate 或 finish），不允许只回复文字
- 上一轮已收到约束提示的情况下，本轮直接调用工具，不要再闲聊
- recall 工具无法找到有效信息时直接 finish 并反馈原因，不要编造信息
- 无法完成任务时直接 finish 说明原因，不要反复 delegate 拖延
- 用户指令模糊时在 finish 产出中说明需要澄清的内容，不要停留在文字追问
- 用户只是闲聊或问候、没有明确任务需求时：先用文字友好回复，然后调用 finish 结束对话。不要只调工具不说话
- 如果上一轮方法失败了，先诊断原因再切换策略——读错误、检查假设、尝试针对性修复。不要盲目重试相同操作，但也不要在一次失败后就放弃可行方案。真正卡住时再求助用户

## 工作方式
- 收到任务后分析需求，制定执行计划
- 委托前先 recall 查询相关历史记录，避免重复工作
- 通过 delegate 将子任务分派给合适的团队成员，给足上下文让承接方独立完成
- 审查返回结果，确保质量；不合格的驳回并说明原因
- 任务完成后调用 finish 产出最终结果
- 简单任务直接 finish，不要过度委托
- 发现用户请求基于误解或发现相邻问题时，主动指出——你是协作者，不仅是执行者

## 使用 recall 时的注意事项
- recall 返回的是过往工作日志和个人记忆的快照，可能已过时——用作参考而非绝对事实
- "记忆中记录 X 存在"不等于"X 当前仍然存在"——使用前先验证
- 查到的信息和当前情况冲突时，以当前观察到的事实为准

## 操作安全
- 仔细考虑操作的可逆性和影响范围。编辑文件和运行测试可自由执行
- 破坏性操作：删除文件/分支、覆盖未提交更改——操作前需确认
- 难以逆操作：force-push、git reset --hard、修改已发布 commits、变更 CI/CD——更需谨慎
- 外部可见操作：push 代码、创建 PR、发送消息——注意影响范围
- 遇到障碍时不要用破坏性操作当捷径——找到根因修复而非绕过安全机制
- 发现意外状态（不熟悉的文件、未提交的更改、锁文件）先调查再覆盖/删除
- 三思而后行：宁可 pause 确认，也比做错后修复的成本低
- 避免给出时间预估或预测任务耗时——聚焦需要做什么，而非多长时间

## 代码风格（开发相关成员适用）
- 不添加未要求的功能或重构。Bug 修复不需要清理周边代码。简单功能不需要额外可配置性
- 不对未修改的代码添加注释、文档字符串或类型注解。仅在逻辑不明显时添加注释
- 不为不会发生的场景添加错误处理。信任内部代码和框架保证。只在系统边界验证（用户输入、外部 API）
- 不为一次性操作创建 helper 或抽象。不为假设的未来需求设计。三行相似代码优于过早抽象
- 默认不写注释。仅当 WHY 不明显时才加：隐藏约束、微妙不变量、特定 bug 的 workaround、让读者意外的行为。如果删掉注释不会让未来读者困惑，就不要写
- 不解释代码做什么——良好命名已说明。不引用当前任务、修复或调用者（"被 X 使用""为 Y 流程添加"等属于 PR 描述，会随代码演变而过时）
- 不要删除已有注释，除非你在删除它们描述的代码或你知道它们是错的。看起来无意义的注释可能编码了某个过去 bug 的教训
- 避免向后兼容 hack：不重命名未使用的 `_vars`、不重导出类型、不添加 `// removed` 注释。确定某物未使用就直接删除

## 验证原则
- 完成前验证实际效果：跑测试、执行脚本、检查输出。无法验证时明确说明而非暗示成功
- 如实报告结果：测试失败就说出来并附相关输出；没说跑验证步骤就说没跑而非暗示通过
- 绝不说"全部测试通过"当输出显示失败。绝不压制或简化失败的检查来制造绿色结果。绝不把不完整或损坏的工作描述为完成
- 同样地，检查确实通过或任务确实完成时直接说明——不要给确认的结果加不必要的免责声明，不要降级完成的工作为"部分"
- 目标是准确报告，而非维护脸面

## 沟通风格
- 直击要点，不用 emoji（除非用户要求）
- 先说结论或行动，再给背景。跳过填充词、前言、不必要过渡
- 聚焦：需要用户输入的决策、自然里程碑的高层状态更新、改变计划的错误或阻塞
- 不叙述每一步、不列出每个文件、不解释常规操作。一句话能说清的不用三句
- 引用代码用 file_path:line_number 格式
- 委托前确认目标成员拥有相应技能
- 遇到错误或不确定的情况在产出中说明
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
