# AgentImageLoaderPlugin

## 类型

公开：
```text
AgentImageLoaderPlugin：AgentImage加载插件，公开结构体--配置镜像根目录、加载限制和处理Schedule
    root: PathBuf--agent-images根目录，私有
    schedule: String--请求准备、异步响应与Entity提交所属Schedule，私有
    limits: AgentImageLoaderLimits--单个镜像加载限制，私有
    open(root: impl Into<PathBuf>) -> Result<Self, AgentImageLoadError>
        打开镜像库：公开关联函数，规范化root并确保镜像根目录存在
        行为：使用默认限制和RuntimePlugin::PRE_UPDATE，不扫描或加载具体镜像
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，使用schedule替换默认Schedule并返回自身
    impl Plugin for AgentImageLoaderPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装AgentImage加载状态、异步读取System和主线程提交System
            行为：
                确认RuntimePlugin和AsyncRuntimePlugin已安装
                确认schedule存在且AgentImageLoaderPluginInstalled尚未安装
                插入AgentImageLoaderPluginInstalled和AgentImageLoaderState
                挂载prepare_agent_image_load_system
                通过add_async_system挂载AgentImageReadTask处理器
                挂载apply_agent_image_load_system

AgentImageLoaderPluginInstalled：镜像加载插件安装标记，公开单元Resource--供WorkspacePlugin确认依赖并阻止重复安装
    impl Resource for AgentImageLoaderPluginInstalled
        Resource：公开trait实现

LoadAgentImage：加载AgentImage，公开事件--请求读取当前磁盘中的逻辑镜像
    id: String--调用方生成的请求ID，用于配对结果
    reference: AgentImageReference--scope/name:tag镜像引用，省略tag时已规范化为latest
    impl Event for LoadAgentImage
        Event：公开trait实现

LoadAgentImageResult：加载AgentImage结果，公开事件--每个已读取请求对应一个结果
    id: String--原请求ID
    reference: AgentImageReference--原镜像引用
    result: Result<Entity, AgentImageLoadError>--成功时返回当前AgentImage Entity
    impl Event for LoadAgentImageResult
        Event：公开trait实现

AgentImageIdentity：AgentImage身份，公开组件--标记Entity代表哪个逻辑镜像
    reference: AgentImageReference--规范化scope/name:tag引用，私有
    reference(&self) -> &AgentImageReference
        取得引用：公开方法，返回镜像引用
    impl Component for AgentImageIdentity
        Component：公开trait实现

AgentImageSoul：AgentImage Soul，公开组件--保存已经过UTF-8和大小验证的完整Soul
    content: Arc<str>--Soul文本，私有
    as_str(&self) -> &str
        取得Soul：公开方法，返回Soul文本引用
    impl Component for AgentImageSoul
        Component：公开trait实现

AgentImageModelParameters：AgentImage模型参数文档，公开结构体--中立保存agent.toml中的可选推理参数
    temperature: Option<f32>--采样温度原始值，私有
    max_output_tokens: Option<u32>--最大输出token数原始值，私有
    top_p: Option<f32>--核采样参数原始值，私有
    stop: Arc<[String]>--停止序列原始值，私有
    temperature(&self) -> Option<f32>
        取得温度：公开方法，返回agent.toml中的原始可选值
    max_output_tokens(&self) -> Option<u32>
        取得输出上限：公开方法，返回agent.toml中的原始可选值
    top_p(&self) -> Option<f32>
        取得核采样参数：公开方法，返回agent.toml中的原始可选值
    stop(&self) -> &[String]
        取得停止序列：公开方法，返回agent.toml中的停止序列只读切片

AgentImageModelConfig：AgentImage模型配置，公开组件--中立保存模型ID文本和模型参数文档
    model: Arc<str>--稳定模型ID文本，私有
    parameters: AgentImageModelParameters--原始模型参数文档，私有
    model(&self) -> &str
        取得模型ID：公开方法，返回模型ID文本，不解释路由语义
    parameters(&self) -> &AgentImageModelParameters
        取得模型参数：公开方法，返回中立参数文档
    impl Component for AgentImageModelConfig
        Component：公开trait实现

AgentImageDefaultVisibility：AgentImage默认资源可见性，公开组件--只读保存镜像默认资源名称
    resources: BTreeSet<ResourceRef>--镜像默认可见的普通Tool、Skill、Workflow统一引用，私有
    resources(&self) -> impl Iterator<Item = &ResourceRef> + '_
        遍历资源：公开方法，按provider和逻辑名称稳定返回
    impl Component for AgentImageDefaultVisibility
        Component：公开trait实现

AgentImageLoadErrorKind：AgentImage加载错误分类，公开枚举
    InvalidRoot
    InvalidRequest
    NotFound
    InvalidLayout
    SymlinkNotAllowed
    LimitExceeded
    SourceChanged
    ManifestReadFailed
    ManifestDecodeFailed
    UnsupportedSchema
    InvalidModelConfig
    SoulReadFailed
    SoulInvalidUtf8
    InvalidResourceName
    TaskPanicked

AgentImageLoadError：AgentImage加载错误，公开结构体--提供稳定分类和不暴露绝对路径的安全描述
    kind: AgentImageLoadErrorKind--错误分类
    message: String--有界安全描述
    new(kind: AgentImageLoadErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存kind和安全描述
        行为：message超过512 UTF-8字节时在字符边界截断并追加省略号，最终长度不超过512字节
    invalid_root(message: impl Into<String>) -> Self
        构造根错误：私有关联函数，以InvalidRoot分类调用new
    kind(&self) -> AgentImageLoadErrorKind
        取得分类：公开方法，返回kind
    message(&self) -> &str
        取得描述：公开方法，返回message引用
    impl Clone for AgentImageLoadError
        Clone：公开trait实现，允许一个合并读取结果回答多个等待者
    impl fmt::Display for AgentImageLoadError
        Display：公开trait实现，只输出kind和有界message
    impl std::error::Error for AgentImageLoadError
        Error：公开trait实现
```

crate公开：
```text
AgentImageLoaderState：AgentImage加载状态，crate公开Resource--保存根目录、当前Entity和正在合并的请求
    root: Arc<PathBuf>--agent-images根目录
    limits: AgentImageLoaderLimits--加载限制
    entities: HashMap<AgentImageReference, Entity>--每个逻辑镜像当前的Entity
    pending: HashMap<AgentImageReference, Vec<String>>--同一镜像正在进行的请求ID
    impl Resource for AgentImageLoaderState
        Resource：crate公开trait实现
```

私有：
```text
AgentImageManifest：AgentImage清单，私有结构体--agent.toml反序列化对象
    schema_version: u32--清单版本，第一版只接受1
    inference: AgentImageModelDocument--模型配置文档

AgentImageModelDocument：AgentImage模型配置文档，私有结构体--只表示agent.toml字段
    model: String--稳定模型ID文本
    temperature: Option<f32>--可选采样温度
    max_output_tokens: Option<u32>--可选最大输出token数
    top_p: Option<f32>--可选核采样参数
    stop: Vec<String>--停止序列，缺省为空

AgentImageLoaderLimits：AgentImage加载限制，私有结构体--限制单个镜像目录的文件数量和内容大小
    max_manifest_bytes: u64--agent.toml最大字节数
    max_soul_bytes: u64--SOUL.md最大字节数
    max_embedded_resources: usize--镜像内置Skill和Workflow名称总上限
    max_model_id_bytes: usize--模型ID最大UTF-8字节数
    max_stop_sequences: usize--停止序列数量上限，仅保护资源读取
    max_stop_sequence_bytes: usize--单个停止序列最大UTF-8字节数，仅保护资源读取
    impl Default for AgentImageLoaderLimits
        Default：私有trait实现，使用64KiB清单、1MiB Soul、4096个资源名称、1KiB模型ID、128个停止序列和4KiB单序列限制
        default() -> Self
            构造默认限制：返回上述固定限制

AgentImageReadTask：AgentImage异步读取任务，私有事件--不持有World引用
    reference: AgentImageReference--目标镜像引用
    root: Arc<PathBuf>--镜像库根目录
    limits: AgentImageLoaderLimits--当前限制快照
    impl Event for AgentImageReadTask
        Event：私有trait实现

PreparedAgentImage：已准备AgentImage，私有结构体--镜像静态数据读取与名称发现均已完成
    reference: AgentImageReference--镜像引用
    soul: AgentImageSoul--已验证Soul
    model: AgentImageModelConfig--中立模型配置
    default_visibility: AgentImageDefaultVisibility--默认只读资源可见性

AgentImageReadPayload：AgentImage读取载荷，私有结构体--无论成功失败都保留镜像引用
    reference: AgentImageReference--原镜像引用
    result: Result<PreparedAgentImage, AgentImageLoadError>--读取结果

AgentImageReadOutput：AgentImage读取输出，私有结构体--允许提交System从共享事件引用中取得一次载荷所有权
    payload: Mutex<Option<AgentImageReadPayload>>--尚未提交的读取载荷
    new(payload: AgentImageReadPayload) -> Self
        构造输出：私有关联函数，将payload保存为尚未取得的载荷
    take(&self) -> Option<AgentImageReadPayload>
        取得载荷：私有方法，从共享事件中取出一次载荷所有权；已取出时返回空

AgentImageTaskError：AgentImage异步监督错误，私有结构体--表示AsyncRuntime整体取消处理器
    source: AsyncTaskError--异步运行时错误
    impl From<AsyncTaskError> for AgentImageTaskError
        From<AsyncTaskError>：私有trait实现，满足add_async_system错误约束
        from(source: AsyncTaskError) -> Self
            转换监督错误：保存source

DirectoryEntryKind：目录入口类型，私有枚举--目录快照只区分Loader允许的普通文件和目录
    File
    Directory

DirectoryEntrySignature：目录入口签名，私有结构体--用于发现加载期间的目录结构变化
    name: OsString--入口名称
    kind: DirectoryEntryKind--入口类型

DirectorySignature：目录签名，私有结构体--按名称排序的直接子项快照
    entries: Vec<DirectoryEntrySignature>--直接子项

FileSignature：文件签名，私有结构体--用于识别静态文件读取期间的可见变化
    length: u64--文件长度
    modified: Option<SystemTime>--文件系统可用时的修改时间
```

## 函数

私有：
```text
prepare_agent_image_load_system(world: &mut World)
    准备镜像加载：私有System，读取LoadAgentImage并合并同一镜像的并发读取
    行为：对每个请求依次执行
        id为空时立即发送InvalidRequest结果
        reference已存在于pending时只把id追加到等待列表
        reference没有pending时插入包含id的等待列表
        克隆root、limits和reference组成AgentImageReadTask
        调用WorldAsyncExt::send_async_event提交异步读取

read_agent_image(task: AgentImageReadTask) -> Result<AgentImageReadOutput, AgentImageTaskError>
    读取镜像：私有异步函数，在AsyncRuntime中读取并准备完整AgentImage
    行为：
        在panic捕获边界内调用read_agent_image_inner
        调用validate_image_layout检查顶层目录结构
        有界读取并解析agent.toml
        schema_version不是1时返回UnsupportedSchema
        验证model非空、无控制字符且不超过读取上限
        原样构造AgentImageModelParameters，不判断推理参数业务范围
        有界读取SOUL.md并验证UTF-8、非空和max_soul_bytes
        调用discover_resource_names分别发现skills与workflows下的scope/name目录
        构造字段私有的AgentImageDefaultVisibility
        不读取SKILL.md、Workflow正文、脚本、模板或资产
        成功或普通失败均包装为保留reference的AgentImageReadPayload和AgentImageReadOutput
        panic转换为带reference和固定安全描述的TaskPanicked输出
        Runtime整体取消时返回AgentImageTaskError

read_agent_image_inner(task: AgentImageReadTask) -> Result<PreparedAgentImage, AgentImageLoadError>
    执行镜像读取：私有异步函数，完成目录验证、静态文件读取、清单解析和资源名称发现
    行为：读取前后比较顶层目录与静态文件签名，成功时构造PreparedAgentImage

validate_model_document(
    document: &AgentImageModelDocument,
    limits: &AgentImageLoaderLimits,
) -> Result<(), AgentImageLoadError>
    验证模型文档：私有函数，只检查模型ID、停止序列数量和单序列读取上限
    行为：不判断temperature、top_p、max_output_tokens或停止序列的业务语义

apply_agent_image_load_system(world: &mut World)
    提交镜像：私有System，读取异步结果并创建或刷新AgentImage Entity
    行为：对每个异步响应依次执行
        AgentImageTaskError只在Runtime取消等无法继续路径写system log
        对每个AgentImageReadOutput调用take取得一次AgentImageReadPayload
        从pending移除payload.reference并取得全部等待id
        output失败时为每个等待id发送克隆的AgentImageLoadError
        output成功且已登记Entity仍存活时替换Identity、Soul、ModelConfig和DefaultVisibility组件
        没有存活Entity时spawn并插入全部组件，再登记reference到Entity
        只有全部PreparedAgentImage已完成结构验证后才修改World
        为每个等待id发送同一reference和Entity的LoadAgentImageResult::Ok

apply_agent_image_payload(world: &mut World, payload: AgentImageReadPayload)
    提交镜像载荷：私有函数，取得等待请求并原子选择成功或失败路径
    行为：成功时复用存活Entity或创建新Entity并全量替换四个组件；失败时只发送克隆错误

resolve_image_root(root: &Path, reference: &AgentImageReference) -> Result<PathBuf, AgentImageLoadError>
    解析镜像目录：私有异步函数，将规范化引用映射到root/scope/name/tag
    行为：规范化结果必须位于root内；不存在返回NotFound；symlink返回SymlinkNotAllowed，非目录返回InvalidLayout

validate_image_layout(root: &Path) -> Result<DirectorySignature, AgentImageLoadError>
    验证镜像布局：私有异步函数，检查AgentImage顶层只包含规定文件与目录并返回快照
    行为：
        agent.toml和SOUL.md必须是普通文件
        skills与workflows缺失时视为空，存在时必须是普通目录
        顶层symlink返回SymlinkNotAllowed，设备文件和未知入口返回InvalidLayout
        不递归验证Skill与Workflow内容，它们由对应Loader Plugin在每次使用时重新验证

discover_resource_names(root: &Path, limits: &AgentImageLoaderLimits) -> Result<BTreeSet<ResourceName>, AgentImageLoadError>
    发现资源名称：私有异步函数，读取root下的scope/name二级目录并返回逻辑名称集合
    行为：
        root不存在时返回空集合
        scope与name必须满足ResourceName规则
        scope和name入口必须是普通目录且不能是symlink
        不进入name目录读取任何资源内容
        名称数量超过max_embedded_resources时返回LimitExceeded
        目录在发现过程中变化时返回SourceChanged

normalize_root(root: PathBuf) -> Result<PathBuf, AgentImageLoadError>
    规范化根：私有函数，要求绝对路径、拒绝父级跳转并移除当前目录段

ensure_root(root: &Path) -> Result<(), AgentImageLoadError>
    确保根存在：私有函数，创建缺失目录后重新检查最终节点
    行为：最终节点是symlink或不是目录时返回InvalidRoot

check_directory(path: &Path, root: bool) -> Result<Option<PathBuf>, AgentImageLoadError>
    检查目录：私有异步函数，区分不存在、合法目录与无效镜像来源
    行为：拒绝symlink和非目录；root决定非目录被分类为InvalidRoot或InvalidLayout

directory_signature(path: &Path, maximum_entries: usize) -> Result<DirectorySignature, AgentImageLoadError>
    获取目录签名：私有异步函数，读取并排序直接子项
    行为：拒绝symlink、特殊文件和超限入口，只在签名中保留入口名称与普通文件或目录类型

read_bounded(
    path: &Path,
    maximum: u64,
    read_error: AgentImageLoadErrorKind,
) -> Result<(Vec<u8>, FileSignature), AgentImageLoadError>
    有界读取：私有异步函数，在读取前后比较文件签名和实际字节数
    行为：最多读取maximum加一字节，拒绝超限和读取中变化，成功时返回原始字节与读取前签名

file_signature(path: &Path, read_error: AgentImageLoadErrorKind) -> Result<FileSignature, AgentImageLoadError>
    获取文件签名：私有异步函数，拒绝symlink和非普通文件并读取长度与修改时间

has_parent(path: &Path) -> bool
    检查父级跳转：私有函数，返回路径是否包含ParentDir组件
```

## 逻辑

```text
安装：
    AgentImageLoaderPlugin::open(agent_images_root)
        -> 验证并创建根目录
        -> app.add_plugin(agent_image_loader_plugin)
        -> 插入空AgentImageLoaderState
        -> 挂载请求准备、异步读取和主线程提交System
        -> 不扫描具体镜像，不创建AgentImage Entity

Workspace启动：
    WorkspacePlugin接收Compose产生的WorkspaceDefinition
        -> 为每个Agent发送LoadAgentImage { id, reference }
        -> Loader按reference合并并发读取
        -> AsyncRuntime读取和结构验证磁盘镜像
        -> 回到主线程创建或刷新AgentImage Entity
        -> 发送LoadAgentImageResult { id, reference, result }
    WorkspacePlugin收到全部成功结果
        -> 读取AgentImageIdentity、Soul、ModelConfig和DefaultVisibility
        -> InferencePlugin把ModelConfig转换为AgentInferenceSnapshot并验证当前路由
        -> 根据DefaultVisibility和Workspace配置构造AgentInstance最终资源可见性
        -> 所有资源准备完成后创建AgentInstance Entity

Workspace重载：
    再次发送LoadAgentImage
        -> 总是重新读取权威磁盘目录
        -> 全量替换当前AgentImage Entity的静态组件
        -> 新AgentInstance取得新的静态配置与资源可见性
        -> 旧AgentInstance的Soul、推理快照和资源可见性保持不变

资源动态使用：
    AgentPlugin在AgentInstance上保存AgentDefaultVisibility和AgentDynamicVisibility
        -> AgentDefaultVisibility保存Workspace创建时合并出的统一ResourceRef集合
        -> AgentDynamicVisibility初始复制默认值，后续可由Agent/Workflow逻辑调整
        -> WorkspacePlugin根据项目目录和AgentImageIdentity构造AgentToolEnvironment
        -> 每次LLM请求由AgentPlugin遍历动态可见资源并逐个交给ToolPlugin生成Tool
        -> 普通Tool、Skill和Workflow均通过ToolDefinitionProvider进入同一tools列表
        -> 资源handler从ToolContext取得项目根和镜像根
        -> 查找顺序固定为项目级、镜像内置、主目录
        -> 新增全新逻辑名称需要workspace reload
        -> 已可见名称的内容修改和同名来源变化在下一次使用时生效

同一镜像并发加载：
    第一条请求创建pending并启动一次异步读取
    后续请求只追加自己的id
    一次读取完成后全部等待者收到各自id对应的相同结果

失败：
    任一文件或结构验证失败
        -> 不创建新Entity
        -> 不修改已有Entity
        -> 每个等待者收到AgentImageLoadError
        -> WorkspacePlugin终止本次Workspace启动或重载
```

## 持有关系

```text
App
└── World
    ├── AgentImageLoaderState Resource
    │   ├── root: Arc<PathBuf>
    │   ├── limits: AgentImageLoaderLimits
    │   ├── entities: HashMap<AgentImageReference, Entity>
    │   └── pending: HashMap<AgentImageReference, Vec<String>>
    └── AgentImage Entity
        ├── AgentImageIdentity
        │   └── reference: AgentImageReference
        ├── AgentImageSoul
        │   └── content: Arc<str>
        ├── AgentImageModelConfig
        │   ├── model: Arc<str>
        │   └── parameters: AgentImageModelParameters
        └── AgentImageDefaultVisibility
            └── resources: BTreeSet<ResourceRef>

异步读取期间：
AgentImageReadTask
├── reference: AgentImageReference
├── root: Arc<PathBuf>
└── limits: AgentImageLoaderLimits
    -> AgentImageReadOutput
       └── payload: Mutex<Option<AgentImageReadPayload>>
           ├── reference: AgentImageReference
           └── result: Result<PreparedAgentImage, AgentImageLoadError>
               └── PreparedAgentImage
                   ├── reference: AgentImageReference
                   ├── soul: AgentImageSoul
                   ├── model: AgentImageModelConfig
                   └── default_visibility: AgentImageDefaultVisibility
```
