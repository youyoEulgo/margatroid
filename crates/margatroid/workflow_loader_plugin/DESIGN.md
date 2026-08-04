# WorkflowLoaderPlugin

## 类型

公开：
```text
WorkflowLoaderPlugin：Workflow加载插件，公开结构体--配置主目录Workflow根和处理Schedule
    home_root: PathBuf--~/.margatroid/workflows或daemon指定的等价目录，私有
    schedule: String--请求准备、异步响应与结果发布所属Schedule，私有
    limits: WorkflowLoaderLimits--Workflow读取限制，私有
    open(home_root: impl Into<PathBuf>) -> Result<Self, WorkflowLoadError>
        打开Workflow根：公开关联函数，规范化home_root并确保目录存在
        行为：使用RuntimePlugin::PRE_UPDATE和默认限制，不扫描或预加载具体Workflow
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，使用schedule替换默认Schedule并返回自身
    impl Plugin for WorkflowLoaderPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装WorkflowLoaderState、请求System、异步读取System和结果发布System
            行为：
                确认RuntimePlugin和AsyncRuntimePlugin已安装
                确认schedule存在且WorkflowLoaderState尚未安装
                插入WorkflowLoaderState
                挂载prepare_workflow_load_system
                通过add_async_system挂载WorkflowReadTask处理器
                挂载publish_workflow_load_system

WorkflowVisibility：Workflow可见性，公开组件--挂在AgentInstance上，只保存当前可见逻辑名称集合
    names: BTreeSet<ResourceName>--当前AgentInstance可见Workflow逻辑名称，私有
    new() -> Self
        构造可见性：公开关联函数，返回空名称集合
    with(mut self, names: impl IntoIterator<Item = ResourceName>) -> Self
        启用Workflow：公开方法，将names去重加入可见集合并返回自身
    without(mut self, names: impl IntoIterator<Item = ResourceName>) -> Self
        禁用Workflow：公开方法，将names从可见集合移除并返回自身
    contains(&self, name: &ResourceName) -> bool
        检查可见：公开方法，返回name是否属于当前可见集合
    names(&self) -> impl Iterator<Item = &ResourceName> + '_
        遍历名称：公开方法，按ResourceName顺序返回可见Workflow
    impl Default for WorkflowVisibility
        Default：公开trait实现，与new等价
    impl Component for WorkflowVisibility
        Component：公开trait实现

WorkflowSourceRoots：Workflow来源根，公开组件--挂在AgentInstance上，只保存动态加载所需的实例级位置
    project_root: Arc<PathBuf>--<project>/.margatroid/workflows，私有
    image_root: Arc<PathBuf>--<agent-image>/workflows，私有
    new(project_root: PathBuf, image_root: PathBuf) -> Result<Self, WorkflowLoadError>
        构造来源根：公开关联函数，规范化项目级和AgentImage内置Workflow根
        行为：两个根都必须是绝对路径且不包含父级跳转，不读取任何Workflow内容
    impl Component for WorkflowSourceRoots
        Component：公开trait实现

LoadWorkflow：加载Workflow，公开事件--请求加载某个AgentInstance当前可见的Workflow
    id: String--调用方生成的请求ID，用于配对结果
    agent: Entity--请求使用Workflow的AgentInstance Entity
    name: ResourceName--需要加载的逻辑Workflow名称
    impl Event for LoadWorkflow
        Event：公开trait实现

LoadWorkflowResult：加载Workflow结果，公开事件--保留原请求路由并返回本次读取内容
    id: String--原请求ID
    agent: Entity--原AgentInstance Entity
    name: ResourceName--原Workflow逻辑名称
    result: Result<LoadedWorkflow, WorkflowLoadError>--本次动态读取结果
    impl Event for LoadWorkflowResult
        Event：公开trait实现

WorkflowSource：Workflow来源，公开枚举--描述本次加载实际命中的作用域
    Project--项目级.margatroid/workflows
    Image--AgentImage内置workflows
    Home--主目录~/.margatroid/workflows

WorkflowManifest：Workflow清单，公开结构体--保存入口、说明和Skill依赖等稳定元数据
    name: ResourceName--清单中的逻辑名称，私有
    description: Arc<str>--Workflow描述，私有
    entry: Arc<PathBuf>--入口文件相对于Workflow根目录的路径，私有
    skill_dependencies: Arc<[ResourceName]>--入口执行需要的Skill逻辑名称，私有
    name(&self) -> &ResourceName
        取得名称：公开方法，返回清单中的逻辑名称
    description(&self) -> &str
        取得描述：公开方法，返回Workflow描述
    entry(&self) -> &Path
        取得入口：公开方法，返回入口相对路径
    skill_dependencies(&self) -> impl Iterator<Item = &ResourceName> + '_
        遍历Skill依赖：公开方法，按清单顺序返回去重后的Skill逻辑名称

LoadedWorkflow：已加载Workflow，公开结构体--一次LoadWorkflow取得的当前清单、入口内容和来源
    name: ResourceName--逻辑Workflow名称，私有
    source: WorkflowSource--本次实际命中来源，私有
    root: Arc<PathBuf>--本次命中的规范化Workflow根目录，私有
    manifest: WorkflowManifest--已解析Workflow清单，私有
    entry_bytes: Arc<[u8]>--入口文件当前内容，私有
    name(&self) -> &ResourceName
        取得名称：公开方法，返回逻辑Workflow名称
    source(&self) -> WorkflowSource
        取得来源：公开方法，返回本次命中的作用域
    manifest(&self) -> &WorkflowManifest
        取得清单：公开方法，返回本次读取的Workflow元数据
    entry_bytes(&self) -> &[u8]
        取得入口内容：公开方法，返回入口文件的只读字节
    resolve(&self, relative: PathBuf) -> impl Future<Output = Result<PathBuf, WorkflowLoadError>> + Send + 'static
        解析辅助路径：公开异步方法，将Workflow引用的相对路径安全解析到本次命中目录
        行为：
            relative必须是非空相对路径且不包含父级跳转
            克隆root并返回执行文件系统检查的异步Future
            Future逐级检查当前路径，拒绝symlink和特殊文件
            最终路径必须仍位于root内且当前存在
            返回可交给SandboxPlugin或其他文件消费者的规范化路径

WorkflowLoadErrorKind：Workflow加载错误分类，公开枚举
    InvalidRoot
    InvalidRequest
    AgentNotAlive
    VisibilityMissing
    SourceRootsMissing
    NotVisible
    NotFound
    InvalidSource
    SymlinkNotAllowed
    ManifestReadFailed
    ManifestDecodeFailed
    SchemaUnsupported
    NameMismatch
    DescriptionInvalid
    EntryMissing
    EntryReadFailed
    EntryTooLarge
    InvalidDependency
    SourceChanged
    TaskPanicked

WorkflowLoadError：Workflow加载错误，公开结构体--提供稳定分类和不暴露绝对路径或Workflow正文的安全描述
    kind: WorkflowLoadErrorKind--错误分类
    message: String--有界安全描述
    new(kind: WorkflowLoadErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存kind和安全描述
        行为：message超过512 UTF-8字节时在字符边界截断并追加省略号，最终长度不超过512字节
    invalid_root(message: impl Into<String>) -> Self
        构造根错误：私有关联函数，以InvalidRoot分类调用new
    kind(&self) -> WorkflowLoadErrorKind
        取得分类：公开方法，返回kind
    message(&self) -> &str
        取得描述：公开方法，返回message引用
    impl fmt::Display for WorkflowLoadError
        Display：公开trait实现，只输出kind和有界message
    impl std::error::Error for WorkflowLoadError
        Error：公开trait实现
```

crate公开：
```text
WorkflowLoaderState：Workflow加载状态，crate公开Resource--保存主目录根和私有读取限制
    home_root: Arc<PathBuf>--主目录Workflow根
    limits: WorkflowLoaderLimits--Workflow读取限制
    impl Resource for WorkflowLoaderState
        Resource：crate公开trait实现
```

私有：
```text
WorkflowLoaderLimits：Workflow加载限制，私有结构体--限制清单、入口和依赖的读取大小
    max_manifest_bytes: u64--workflow.toml最大字节数
    max_entry_bytes: u64--入口文件最大字节数
    max_description_bytes: usize--description最大UTF-8字节数
    max_dependencies: usize--Skill依赖数量上限
    impl Default for WorkflowLoaderLimits
        Default：私有trait实现，使用有界清单、入口、描述和依赖限制

WorkflowManifestDocument：Workflow清单文档，私有结构体--workflow.toml反序列化对象
    schema_version: u32--清单版本，第一版只接受1
    name: String--不含scope的Workflow名称
    description: String--Workflow描述
    entry: String--入口文件相对路径
    skills: Vec<String>--入口执行依赖的Skill逻辑名称

WorkflowReadTask：Workflow异步读取任务，私有事件--主线程已完成Agent和可见性检查
    id: String--原请求ID
    agent: Entity--原AgentInstance Entity
    name: ResourceName--目标Workflow名称
    project_root: Arc<PathBuf>--项目级查找根
    image_root: Arc<PathBuf>--镜像内置查找根
    home_root: Arc<PathBuf>--主目录查找根
    limits: WorkflowLoaderLimits--读取限制快照
    impl Event for WorkflowReadTask
        Event：私有trait实现

WorkflowReadPayload：Workflow异步读取载荷，私有结构体--成功失败都保留完整请求路由
    id: String--原请求ID
    agent: Entity--原AgentInstance Entity
    name: ResourceName--原Workflow名称
    result: Result<LoadedWorkflow, WorkflowLoadError>--领域读取结果

WorkflowReadOutput：Workflow异步读取输出，私有结构体--允许发布System从共享事件引用中取得一次载荷所有权
    payload: Mutex<Option<WorkflowReadPayload>>--尚未发布的读取载荷
    new(payload: WorkflowReadPayload) -> Self
        构造输出：私有关联函数，将payload保存为尚未取得的载荷
    take(&self) -> Option<WorkflowReadPayload>
        取得载荷：私有方法，从共享事件中取出一次载荷所有权；已取出时返回空

WorkflowTaskError：Workflow异步监督错误，私有结构体--表示AsyncRuntime整体取消处理器
    source: AsyncTaskError--异步运行时错误
    impl From<AsyncTaskError> for WorkflowTaskError
        From<AsyncTaskError>：私有trait实现，满足add_async_system错误约束

ResolvedWorkflow：已解析Workflow来源，私有结构体--第一个存在且目录边界有效的候选项
    source: WorkflowSource--命中作用域
    root: PathBuf--规范化Workflow目录

FileSignature：文件签名，私有结构体--用于识别Workflow文件读取期间的可见变化
    length: u64--文件长度
    modified: Option<SystemTime>--文件系统可用时的修改时间
```

## 函数

私有：
```text
prepare_workflow_load_system(world: &mut World)
    准备Workflow加载：私有System，读取LoadWorkflow并在主线程取得可见性和来源根快照
    行为：对每个请求依次执行
        id为空时立即发送InvalidRequest结果
        agent不存活时立即发送AgentNotAlive结果
        agent没有WorkflowVisibility时立即发送VisibilityMissing结果
        name不在visibility.names时立即发送NotVisible结果
        agent没有WorkflowSourceRoots时立即发送SourceRootsMissing结果
        克隆WorkflowSourceRoots的project_root、image_root和WorkflowLoaderState的home_root
        组装WorkflowReadTask并调用WorldAsyncExt::send_async_event

read_workflow(task: WorkflowReadTask) -> Result<WorkflowReadOutput, WorkflowTaskError>
    读取Workflow：私有异步函数，按作用域选择来源并解析当前Workflow包
    行为：
        始终保留task的id、agent和name
        在panic捕获边界内调用read_workflow_inner
        有界读取root/workflow.toml并解析WorkflowManifestDocument
        schema_version不是1时返回SchemaUnsupported
        清单name必须等于ResourceName的name部分
        description去除首尾空白后必须非空且不超过上限
        entry必须是非空相对路径且不包含父级跳转
        skills中的每个名称必须符合ResourceName规则且不能重复
        entry必须位于Workflow根目录内、不能是symlink且必须是普通文件
        有界读取entry文件为entry_bytes，不解析节点类型或执行内容
        成功时构造LoadedWorkflow
        普通失败包装进WorkflowReadOutput.payload.result
        panic转换为带原路由和固定安全描述的TaskPanicked结果
        Runtime整体取消时返回WorkflowTaskError

read_workflow_inner(task: WorkflowReadTask) -> Result<LoadedWorkflow, WorkflowLoadError>
    执行Workflow读取：私有异步函数，完成来源解析、清单解析、入口读取和变化检查
    行为：成功时将本次名称、来源、根、清单与入口内容组装为LoadedWorkflow

publish_workflow_load_system(world: &mut World)
    发布Workflow结果：私有System，读取异步响应并发送LoadWorkflowResult
    行为：
        对每个WorkflowReadOutput调用take取得一次WorkflowReadPayload
        WorkflowReadPayload转换为相同id、agent和name的LoadWorkflowResult
        WorkflowTaskError只在Runtime关闭等无法继续路径写system log，不伪造无法配对的业务结果

resolve_workflow(
    name: &ResourceName,
    project_root: &Path,
    image_root: &Path,
    home_root: &Path,
) -> Result<ResolvedWorkflow, WorkflowLoadError>
    解析Workflow来源：私有异步函数，按固定优先级查找scope/name目录
    行为：
        候选顺序固定为Project、Image、Home
        对每个根拼接name.scope和name.name
        候选不存在时继续下一个来源
        候选存在但任一路径段是symlink、不是目录或越出根时立即返回错误
        返回第一个存在且目录边界有效的候选项
        所有来源都不存在时返回NotFound

parse_workflow_manifest(
    name: &ResourceName,
    source: &str,
    limits: &WorkflowLoaderLimits,
) -> Result<WorkflowManifest, WorkflowLoadError>
    解析Workflow清单：私有函数，解析workflow.toml并构造中立WorkflowManifest
    行为：
        有界反序列化WorkflowManifestDocument
        schema_version只能为1
        document.name必须等于name.name
        description必须非空且不超过限制
        entry转换为不含父级跳转的相对PathBuf
        skills转换为去重的ResourceName集合
        依赖数量、名称和清单字段超过限制时返回对应错误
        返回不读取入口内容的WorkflowManifest

resolve_workflow_path(root: PathBuf, relative: PathBuf) -> Result<PathBuf, WorkflowLoadError>
    解析Workflow辅助路径：私有异步函数，实现LoadedWorkflow::resolve的路径安全规则
    行为：拒绝空路径、绝对路径和父级跳转，拼接后调用ensure_existing_path逐级检查

normalize_root(root: PathBuf) -> Result<PathBuf, WorkflowLoadError>
    规范化根：私有函数，要求绝对路径、拒绝父级跳转并移除当前目录段

ensure_root(root: &Path) -> Result<(), WorkflowLoadError>
    确保根存在：私有函数，创建缺失目录后重新检查最终节点
    行为：最终节点是symlink或不是目录时返回InvalidRoot

check_candidate(root: &Path, name: &ResourceName) -> Result<Option<PathBuf>, WorkflowLoadError>
    检查候选项：私有异步函数，依次检查根、scope目录和name目录
    行为：任一级不存在时返回空，存在但边界无效时返回错误，合法时返回Workflow目录

check_directory(path: &Path, root: bool) -> Result<Option<PathBuf>, WorkflowLoadError>
    检查目录：私有异步函数，区分不存在、合法目录与无效来源
    行为：拒绝symlink和非目录；root决定非目录被分类为InvalidRoot或InvalidSource

ensure_existing_path(root: &Path, path: &Path) -> Result<PathBuf, WorkflowLoadError>
    检查现有路径：私有异步函数，从root开始逐级检查path
    行为：拒绝越界、symlink和特殊文件，允许普通目录或文件并返回最终路径

read_bounded(path: &Path, maximum: u64) -> Result<(Vec<u8>, FileSignature), WorkflowLoadError>
    有界读取：私有异步函数，在读取前后比较文件签名和实际字节数
    行为：拒绝超限和读取中变化，成功时返回原始字节与读取前签名

file_signature(path: &Path) -> Result<FileSignature, WorkflowLoadError>
    获取文件签名：私有异步函数，拒绝symlink和非普通文件并读取长度与修改时间

has_parent(path: &Path) -> bool
    检查父级跳转：私有函数，返回路径是否包含ParentDir组件
```

## 逻辑

```text
安装：
    WorkflowLoaderPlugin::open(~/.margatroid/workflows)
        -> 验证并创建主目录Workflow根
        -> app.add_plugin(workflow_loader_plugin)
        -> 插入WorkflowLoaderState
        -> 挂载请求、异步读取和结果发布System
        -> 不扫描或预加载任何Workflow

创建AgentInstance：
    WorkspacePlugin读取AgentImageDefaultVisibility中的默认Workflow名称
        -> 读取WorkspaceAgentSpec.workflows
        -> 读取WorkspaceAgentSpec.disable_workflows
        -> WorkflowVisibility::new().with(defaults).with(additional).without(disabled)
        -> WorkflowSourceRoots::new(project_root, image_root)
        -> 将WorkflowVisibility和WorkflowSourceRoots挂到新的AgentInstance Entity
    WorkflowVisibility和WorkflowSourceRoots直到workspace reload保持不变

加载Workflow：
    WorkflowPlugin发送LoadWorkflow { id, agent, name }
        -> WorkflowLoaderPlugin读取AgentInstance.WorkflowVisibility
        -> 按项目级、镜像内置、主目录顺序查找Workflow目录
        -> 读取并解析workflow.toml
        -> 读取清单声明的入口文件
        -> 返回LoadedWorkflow { manifest, entry_bytes, source }
        -> WorkflowPlugin取得入口内容并决定如何解释节点

检查Skill依赖：
    WorkflowPlugin读取LoadedWorkflow.manifest().skill_dependencies()
        -> 对每个依赖名称检查当前AgentInstance的SkillVisibility
        -> 通过SkillLoaderPlugin按项目级、镜像内置、主目录顺序读取
        -> 任一依赖不可见、找不到或内容非法时拒绝本次Workflow执行
        -> WorkflowLoaderPlugin不执行依赖检查，不依赖SkillLoaderPlugin

读取辅助资源：
    WorkflowPlugin或SandboxPlugin调用LoadedWorkflow::resolve(relative)
        -> 异步检查路径仍位于本次命中的Workflow根目录
        -> 拒绝父级跳转、symlink、特殊文件和不存在路径
        -> 返回可交给文件消费者的规范化路径

动态修改：
    修改已有可见Workflow的清单、入口、脚本或模板
        -> AgentInstance.WorkflowVisibility不变
        -> 下一次LoadWorkflow重新选择来源并读取最新内容
    添加更高优先级的同名Workflow
        -> 下一次LoadWorkflow自动命中新来源
    添加全新逻辑名称或修改Workspace可见性配置
        -> 当前WorkflowVisibility不变
        -> workspace reload后才可见

失败：
    name不可见
        -> 主线程立即返回NotVisible，不访问磁盘
    项目级同名目录存在但清单或入口损坏
        -> 返回项目级错误，不尝试镜像或主目录
    任一Skill依赖缺失
        -> WorkflowPlugin拒绝本次执行，不静默跳过依赖
    读取期间Workflow文件变化
        -> 返回SourceChanged，由WorkflowPlugin决定是否重新发送
```

## 持有关系

```text
App
└── World
    ├── WorkflowLoaderState Resource
    │   ├── home_root: Arc<PathBuf>
    │   └── limits: WorkflowLoaderLimits
    └── AgentInstance Entity
        ├── WorkflowVisibility
        │   └── names: BTreeSet<ResourceName>
        └── WorkflowSourceRoots
            ├── project_root: Arc<PathBuf>
            └── image_root: Arc<PathBuf>

一次加载期间：
LoadWorkflow
├── id: String
├── agent: Entity
└── name: ResourceName
    -> WorkflowReadTask
       ├── id: String
       ├── agent: Entity
       ├── name: ResourceName
       ├── project_root: Arc<PathBuf>
       ├── image_root: Arc<PathBuf>
       ├── home_root: Arc<PathBuf>
       └── limits: WorkflowLoaderLimits
           -> WorkflowReadOutput
              └── payload: Mutex<Option<WorkflowReadPayload>>
                  ├── id: String
                  ├── agent: Entity
                  ├── name: ResourceName
                  └── result: Result<LoadedWorkflow, WorkflowLoadError>
                      └── LoadedWorkflow
                          ├── name: ResourceName
                          ├── source: WorkflowSource
                          ├── root: Arc<PathBuf>
                          ├── manifest: WorkflowManifest
                          └── entry_bytes: Arc<[u8]>
```
