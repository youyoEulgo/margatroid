# SkillLoaderPlugin

## 类型

公开：
```text
SkillLoaderPlugin：Skill加载插件，公开结构体--配置主目录Skill根和处理Schedule
    home_root: PathBuf--~/.margatroid/skills或daemon指定的等价目录，私有
    schedule: String--命令准备、异步响应与结果发布所属Schedule，私有
    limits: SkillLoaderLimits--SKILL.md读取限制，私有
    open(home_root: impl Into<PathBuf>) -> Result<Self, SkillLoadError>
        打开Skill根：公开关联函数，规范化home_root并确保目录存在
        行为：使用RuntimePlugin::PRE_UPDATE和默认限制，不扫描或预加载具体Skill
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，使用schedule替换默认Schedule并返回自身
    impl Plugin for SkillLoaderPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装SkillLoaderState、请求System、异步读取System和结果发布System
            行为：
                确认RuntimePlugin和AsyncRuntimePlugin已安装
                确认schedule存在且SkillLoaderState尚未安装
                插入SkillLoaderState
                挂载prepare_skill_load_system
                通过add_async_system挂载SkillReadTask处理器
                挂载publish_skill_load_system

SkillVisibility：Skill可见性，公开组件--挂在AgentInstance上，只保存当前可见逻辑名称集合
    names: BTreeSet<ResourceName>--当前AgentInstance可见Skill逻辑名称，私有
    new() -> Self
        构造可见性：公开关联函数，返回空名称集合
    with(mut self, names: impl IntoIterator<Item = ResourceName>) -> Self
        启用Skill：公开方法，将names去重加入可见集合并返回自身
    without(mut self, names: impl IntoIterator<Item = ResourceName>) -> Self
        禁用Skill：公开方法，将names从可见集合移除并返回自身
    contains(&self, name: &ResourceName) -> bool
        检查可见：公开方法，返回name是否属于当前可见集合
    names(&self) -> impl Iterator<Item = &ResourceName> + '_
        遍历名称：公开方法，按ResourceName顺序返回可见Skill
    impl Default for SkillVisibility
        Default：公开trait实现，与new等价
    impl Component for SkillVisibility
        Component：公开trait实现

SkillSourceRoots：Skill来源根，公开组件--挂在AgentInstance上，只保存动态加载所需的实例级位置
    project_root: Arc<PathBuf>--<project>/.margatroid/skills，私有
    image_root: Arc<PathBuf>--<agent-image>/skills，私有
    new(project_root: PathBuf, image_root: PathBuf) -> Result<Self, SkillLoadError>
        构造来源根：公开关联函数，规范化项目级和AgentImage内置Skill根
        行为：两个根都必须是绝对路径且不包含父级跳转，不读取任何Skill内容
    impl Component for SkillSourceRoots
        Component：公开trait实现

LoadSkill：加载Skill，公开事件--请求加载某个AgentInstance当前可见的Skill
    id: String--调用方生成的请求ID，用于配对结果
    agent: Entity--请求使用Skill的AgentInstance Entity
    name: ResourceName--需要加载的逻辑Skill名称
    impl Event for LoadSkill
        Event：公开trait实现

LoadSkillResult：加载Skill结果，公开事件--保留原请求路由并返回本次读取内容
    id: String--原请求ID
    agent: Entity--原AgentInstance Entity
    name: ResourceName--原Skill逻辑名称
    result: Result<LoadedSkill, SkillLoadError>--本次动态读取结果
    impl Event for LoadSkillResult
        Event：公开trait实现

SkillSource：Skill来源，公开枚举--描述本次加载实际命中的作用域
    Project--项目级.margatroid/skills
    Image--AgentImage内置skills
    Home--主目录~/.margatroid/skills

LoadedSkill：已加载Skill，公开结构体--一次LoadSkill取得的当前内容和来源
    name: ResourceName--逻辑Skill名称，私有
    source: SkillSource--本次实际命中来源，私有
    root: Arc<PathBuf>--本次命中的规范化Skill根目录，私有
    description: Arc<str>--frontmatter描述，私有
    instructions: Arc<str>--不含frontmatter的完整SKILL.md正文，私有
    name(&self) -> &ResourceName
        取得名称：公开方法，返回逻辑Skill名称
    source(&self) -> SkillSource
        取得来源：公开方法，返回本次命中的作用域
    description(&self) -> &str
        取得描述：公开方法，返回模型可见描述
    instructions(&self) -> &str
        取得指令：公开方法，返回完整Skill指令正文
    resolve(&self, relative: PathBuf) -> impl Future<Output = Result<PathBuf, SkillLoadError>> + Send + 'static
        解析辅助路径：公开异步方法，将Skill指令引用的相对路径安全解析到本次命中目录
        行为：
            relative必须是非空相对路径且不包含父级跳转
            克隆root并返回执行文件系统检查的异步Future
            Future逐级检查当前路径，拒绝symlink和特殊文件
            最终路径必须仍位于root内且当前存在
            返回可交给SandboxPlugin或其他文件消费者的规范化路径

SkillLoadErrorKind：Skill加载错误分类，公开枚举
    InvalidRoot
    InvalidRequest
    AgentNotAlive
    VisibilityMissing
    SourceRootsMissing
    NotVisible
    NotFound
    InvalidSource
    SymlinkNotAllowed
    ReadFailed
    LimitExceeded
    InvalidUtf8
    FrontmatterMissing
    FrontmatterDecodeFailed
    NameMismatch
    DescriptionInvalid
    InstructionsInvalid
    SourceChanged
    TaskPanicked

SkillLoadError：Skill加载错误，公开结构体--提供稳定分类和不暴露绝对路径或Skill正文的安全描述
    kind: SkillLoadErrorKind--错误分类
    message: String--有界安全描述
    new(kind: SkillLoadErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存kind和安全描述
        行为：message超过512 UTF-8字节时在字符边界截断并追加省略号，最终长度不超过512字节
    invalid_root(message: impl Into<String>) -> Self
        构造根错误：私有关联函数，以InvalidRoot分类调用new
    kind(&self) -> SkillLoadErrorKind
        取得分类：公开方法，返回kind
    message(&self) -> &str
        取得描述：公开方法，返回message引用
    impl fmt::Display for SkillLoadError
        Display：公开trait实现，只输出kind和有界message
    impl std::error::Error for SkillLoadError
        Error：公开trait实现
```

crate公开：
```text
SkillLoaderState：Skill加载状态，crate公开Resource--保存主目录根和私有读取限制
    home_root: Arc<PathBuf>--主目录Skill根
    limits: SkillLoaderLimits--SKILL.md读取限制
    impl Resource for SkillLoaderState
        Resource：crate公开trait实现
```

私有：
```text
SkillLoaderLimits：Skill加载限制，私有结构体--限制单个SKILL.md和frontmatter大小
    max_skill_bytes: u64--SKILL.md最大字节数
    max_frontmatter_bytes: usize--YAML frontmatter最大字节数
    max_description_bytes: usize--description最大UTF-8字节数
    impl Default for SkillLoaderLimits
        Default：私有trait实现，使用1MiB SKILL.md、64KiB frontmatter和8KiB description上限

SkillReadTask：Skill异步读取任务，私有事件--主线程已完成Agent和可见性检查
    id: String--原请求ID
    agent: Entity--原AgentInstance Entity
    name: ResourceName--目标Skill名称
    project_root: Arc<PathBuf>--项目级查找根
    image_root: Arc<PathBuf>--镜像内置查找根
    home_root: Arc<PathBuf>--主目录查找根
    limits: SkillLoaderLimits--读取限制快照
    impl Event for SkillReadTask
        Event：私有trait实现

SkillReadPayload：Skill异步读取载荷，私有结构体--成功失败都保留完整请求路由
    id: String--原请求ID
    agent: Entity--原AgentInstance Entity
    name: ResourceName--原Skill名称
    result: Result<LoadedSkill, SkillLoadError>--领域读取结果

SkillReadOutput：Skill异步读取输出，私有结构体--允许发布System从共享事件引用中取得一次载荷所有权
    payload: Mutex<Option<SkillReadPayload>>--尚未发布的读取载荷
    new(payload: SkillReadPayload) -> Self
        构造输出：私有关联函数，将payload保存为尚未取得的载荷
    take(&self) -> Option<SkillReadPayload>
        取得载荷：私有方法，从共享事件中取出一次载荷所有权；已取出时返回空

SkillTaskError：Skill异步监督错误，私有结构体--表示AsyncRuntime整体取消处理器
    source: AsyncTaskError--异步运行时错误
    impl From<AsyncTaskError> for SkillTaskError
        From<AsyncTaskError>：私有trait实现，满足add_async_system错误约束

SkillFrontmatter：SKILL.md frontmatter，私有结构体--只解释第一版需要的稳定字段
    name: String--不含scope的Skill名称
    description: String--模型可见描述

ResolvedSkill：已解析Skill来源，私有结构体--第一个存在且目录边界有效的候选项
    source: SkillSource--命中作用域
    root: PathBuf--规范化Skill目录

FileSignature：文件签名，私有结构体--用于识别SKILL.md读取期间的可见变化
    length: u64--文件长度
    modified: Option<SystemTime>--文件系统可用时的修改时间
```

## 函数

私有：
```text
prepare_skill_load_system(world: &mut World)
    准备Skill加载：私有System，读取LoadSkill并在主线程取得可见性和来源根快照
    行为：对每个请求依次执行
        id为空时立即发送InvalidRequest结果
        agent不存活时立即发送AgentNotAlive结果
        agent没有SkillVisibility时立即发送VisibilityMissing结果
        name不在visibility.names时立即发送NotVisible结果
        agent没有SkillSourceRoots时立即发送SourceRootsMissing结果
        克隆SkillSourceRoots的project_root、image_root和SkillLoaderState的home_root
        组装SkillReadTask并调用WorldAsyncExt::send_async_event

read_skill(task: SkillReadTask) -> Result<SkillReadOutput, SkillTaskError>
    读取Skill：私有异步函数，按作用域选择来源并解析当前SKILL.md
    行为：
        始终保留task的id、agent和name
        在panic捕获边界内调用resolve_skill
        有界读取root/SKILL.md
        读取前后比较SKILL.md类型、长度和修改标记，变化时返回SourceChanged
        验证UTF-8并调用parse_skill_markdown
        成功时构造LoadedSkill
        普通失败包装进SkillReadOutput.result
        panic转换为带原路由和固定安全描述的TaskPanicked结果
        Runtime整体取消时返回SkillTaskError

read_skill_inner(task: SkillReadTask) -> Result<LoadedSkill, SkillLoadError>
    执行Skill读取：私有异步函数，完成来源解析、有界读取、格式解析和变化检查
    行为：成功时将本次名称、来源、根、描述与指令组装为LoadedSkill

publish_skill_load_system(world: &mut World)
    发布Skill结果：私有System，读取异步响应并发送LoadSkillResult
    行为：
        对每个SkillReadOutput调用take取得一次SkillReadPayload
        SkillReadPayload转换为相同id、agent和name的LoadSkillResult
        SkillTaskError只在Runtime关闭等无法继续路径写system log，不伪造无法配对的业务结果

resolve_skill(
    name: &ResourceName,
    project_root: &Path,
    image_root: &Path,
    home_root: &Path,
) -> Result<ResolvedSkill, SkillLoadError>
    解析Skill来源：私有异步函数，按固定优先级查找scope/name目录
    行为：
        候选顺序固定为Project、Image、Home
        对每个根拼接name.scope和name.name
        候选不存在时继续下一个来源
        候选存在但任一路径段是symlink、不是目录或越出根时立即返回错误
        返回第一个存在且目录边界有效的候选项
        所有来源都不存在时返回NotFound

parse_skill_markdown(
    name: &ResourceName,
    source: &str,
    limits: &SkillLoaderLimits,
) -> Result<(Arc<str>, Arc<str>), SkillLoadError>
    解析Skill文档：私有函数，拆分YAML frontmatter并返回description与instructions
    行为：
        source必须以独占行---开始并包含独占行---结束frontmatter
        frontmatter超过限制时返回LimitExceeded
        反序列化为SkillFrontmatter，未知字段忽略
        frontmatter.name必须等于ResourceName的name部分
        description去除首尾空白后必须非空且不超过上限
        instructions去除分隔符后的单个换行，保留其余正文格式；trim后必须非空
        返回拥有所有权的description与instructions

resolve_skill_path(root: PathBuf, relative: PathBuf) -> Result<PathBuf, SkillLoadError>
    解析Skill辅助路径：私有异步函数，实现LoadedSkill::resolve的路径安全规则
    行为：拒绝空路径、绝对路径和父级跳转，拼接后调用ensure_existing_path逐级检查

normalize_root(root: PathBuf) -> Result<PathBuf, SkillLoadError>
    规范化根：私有函数，要求绝对路径、拒绝父级跳转并移除当前目录段

ensure_root(root: &Path) -> Result<(), SkillLoadError>
    确保根存在：私有函数，创建缺失目录后重新检查最终节点
    行为：最终节点是symlink或不是目录时返回InvalidRoot

check_candidate(root: &Path, name: &ResourceName) -> Result<Option<PathBuf>, SkillLoadError>
    检查候选项：私有异步函数，依次检查根、scope目录和name目录
    行为：任一级不存在时返回空，存在但边界无效时返回错误，合法时返回Skill目录

check_directory(path: &Path, root: bool) -> Result<Option<PathBuf>, SkillLoadError>
    检查目录：私有异步函数，区分不存在、合法目录与无效来源
    行为：拒绝symlink和非目录；root决定非目录被分类为InvalidRoot或InvalidSource

ensure_existing_path(root: &Path, path: &Path) -> Result<PathBuf, SkillLoadError>
    检查现有路径：私有异步函数，从root开始逐级检查path
    行为：拒绝越界、symlink和特殊文件，允许普通目录或文件并返回最终路径

read_bounded(path: &Path, maximum: u64) -> Result<(String, FileSignature), SkillLoadError>
    有界读取：私有异步函数，在读取前后比较文件签名和实际字节数
    行为：拒绝超限、读取中变化和无效UTF-8，成功时返回文本与读取前签名

file_signature(path: &Path) -> Result<FileSignature, SkillLoadError>
    获取文件签名：私有异步函数，拒绝symlink和非普通文件并读取长度与修改时间

has_parent(path: &Path) -> bool
    检查父级跳转：私有函数，返回路径是否包含ParentDir组件
```

## 逻辑

```text
安装：
    SkillLoaderPlugin::open(~/.margatroid/skills)
        -> 验证并创建主目录Skill根
        -> app.add_plugin(skill_loader_plugin)
        -> 插入SkillLoaderState
        -> 挂载请求、异步读取和结果发布System
        -> 不扫描或预加载任何Skill

创建AgentInstance：
    WorkspacePlugin读取AgentImageDefaultVisibility中的默认Skill名称
        -> 读取WorkspaceAgentSpec.skills
        -> 读取WorkspaceAgentSpec.disable_skills
        -> SkillVisibility::new().with(defaults).with(additional).without(disabled)
        -> SkillSourceRoots::new(project_root, image_root)
        -> 将SkillVisibility和SkillSourceRoots挂到新的AgentInstance Entity
    SkillLoaderPlugin不读取AgentImageDefaultVisibility，也不依赖AgentImageLoaderPlugin
    SkillVisibility和SkillSourceRoots直到workspace reload保持不变

准备模型请求：
    SkillPlugin遍历SkillVisibility::names
        -> 为每个名称发送LoadSkill
        -> SkillLoaderPlugin逐个动态解析当前来源和SKILL.md
        -> SkillPlugin收齐LoadedSkill
        -> 使用description生成当前模型可见Skill工具
    任一可见Skill当前无法加载时，本次模型请求失败，不静默隐藏该Skill

调用Skill：
    SkillPlugin收到模型选择的Skill名称
        -> 再次发送LoadSkill，不复用上一次模型请求的正文
        -> 取得调用时最新instructions
        -> 将instructions注入本次调用上下文
        -> 需要辅助文件时调用LoadedSkill::resolve

动态修改：
    修改已有可见Skill的SKILL.md
        -> AgentInstance的SkillVisibility不变
        -> 下一次LoadSkill读取新内容
    添加更高优先级的同名Skill
        -> 下一次LoadSkill自动命中新来源
    添加全新逻辑名称
        -> 当前SkillVisibility不包含该名称
        -> workspace reload后才可见

失败：
    name不可见
        -> 主线程立即返回NotVisible，不访问磁盘
    项目级同名目录存在但损坏
        -> 返回项目级错误，不尝试镜像或主目录
    读取期间SKILL.md变化
        -> 返回SourceChanged，由SkillPlugin决定是否重新发送
```

## 持有关系

```text
App
└── World
    ├── SkillLoaderState Resource
    │   ├── home_root: Arc<PathBuf>
    │   └── limits: SkillLoaderLimits
    └── AgentInstance Entity
        ├── SkillVisibility
        │   └── names: BTreeSet<ResourceName>
        └── SkillSourceRoots
            ├── project_root: Arc<PathBuf>
            └── image_root: Arc<PathBuf>

一次加载期间：
LoadSkill
├── id: String
├── agent: Entity
└── name: ResourceName
    -> SkillReadTask
       ├── id: String
       ├── agent: Entity
       ├── name: ResourceName
       ├── project_root: Arc<PathBuf>
       ├── image_root: Arc<PathBuf>
       ├── home_root: Arc<PathBuf>
       └── limits: SkillLoaderLimits
           -> SkillReadOutput
              └── payload: Mutex<Option<SkillReadPayload>>
                  ├── id: String
                  ├── agent: Entity
                  ├── name: ResourceName
                  └── result: Result<LoadedSkill, SkillLoadError>
                      └── LoadedSkill
                          ├── name: ResourceName
                          ├── source: SkillSource
                          ├── root: Arc<PathBuf>
                          ├── description: Arc<str>
                          └── instructions: Arc<str>
```
