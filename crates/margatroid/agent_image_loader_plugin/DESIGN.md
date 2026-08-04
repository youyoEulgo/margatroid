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
                确认schedule存在且AgentImageLoaderState尚未安装
                插入AgentImageLoaderState
                挂载prepare_agent_image_load_system
                通过add_async_system挂载AgentImageReadTask处理器
                挂载apply_agent_image_load_system

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
    skills: BTreeSet<ResourceName>--镜像默认可见Skill名称，私有
    workflows: BTreeSet<ResourceName>--镜像默认可见Workflow名称，私有
    skills(&self) -> impl Iterator<Item = &ResourceName> + '_
        遍历Skill名称：公开方法，按逻辑名称顺序返回镜像默认可见Skill
    workflows(&self) -> impl Iterator<Item = &ResourceName> + '_
        遍历Workflow名称：公开方法，按逻辑名称顺序返回镜像默认可见Workflow
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
        Default：私有trait实现，使用有界清单、Soul、资源名称、模型ID和停止序列限制

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

AgentImageReadOutput：AgentImage读取输出，私有结构体--无论成功失败都保留镜像引用
    reference: AgentImageReference--原镜像引用
    result: Result<PreparedAgentImage, AgentImageLoadError>--读取结果

AgentImageTaskError：AgentImage异步监督错误，私有结构体--表示AsyncRuntime整体取消处理器
    source: AsyncTaskError--异步运行时错误
    impl From<AsyncTaskError> for AgentImageTaskError
        From<AsyncTaskError>：私有trait实现，满足add_async_system错误约束
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
        在panic捕获边界内调用resolve_image_root
        调用validate_image_layout检查顶层目录结构
        有界读取并解析agent.toml
        schema_version不是1时返回UnsupportedSchema
        验证model非空、无控制字符且不超过读取上限
        原样构造AgentImageModelParameters，不判断推理参数业务范围
        有界读取SOUL.md并验证UTF-8、非空和max_soul_bytes
        调用discover_resource_names分别发现skills与workflows下的scope/name目录
        构造字段私有的AgentImageDefaultVisibility
        不读取SKILL.md、Workflow正文、脚本、模板或资产
        成功或普通失败均包装为保留reference的AgentImageReadOutput
        panic转换为带reference的TaskPanicked输出
        Runtime整体取消时返回AgentImageTaskError

apply_agent_image_load_system(world: &mut World)
    提交镜像：私有System，读取异步结果并创建或刷新AgentImage Entity
    行为：对每个异步响应依次执行
        AgentImageTaskError只在Runtime取消等无法继续路径写system log
        取得AgentImageReadOutput后从pending移除reference并取得全部等待id
        output失败时为每个等待id发送克隆的AgentImageLoadError
        output成功且已登记Entity仍存活时替换Identity、Soul、ModelConfig和DefaultVisibility组件
        没有存活Entity时spawn并插入全部组件，再登记reference到Entity
        只有全部PreparedAgentImage已完成结构验证后才修改World
        为每个等待id发送同一reference和Entity的LoadAgentImageResult::Ok

resolve_image_root(root: &Path, reference: &AgentImageReference) -> Result<PathBuf, AgentImageLoadError>
    解析镜像目录：私有函数，将规范化引用映射到root/scope/name/tag
    行为：规范化结果必须位于root内；不存在返回NotFound；symlink或非目录返回InvalidLayout

validate_image_layout(root: &Path) -> Result<(), AgentImageLoadError>
    验证镜像布局：私有函数，检查AgentImage顶层只包含规定文件与目录
    行为：
        agent.toml和SOUL.md必须是普通文件
        skills与workflows缺失时视为空，存在时必须是普通目录
        顶层symlink、设备文件和未知入口返回InvalidLayout
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
    WorkspacePlugin解析WorkspaceSpec
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

Skill动态加载：
    AgentInstance的SkillVisibility只保存最终可见Skill逻辑名称
        -> WorkspacePlugin根据项目目录和AgentImageIdentity单独构造SkillSourceRoots
        -> 每次准备模型请求或调用Skill时由SkillLoaderPlugin重新读取当前内容
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
            ├── skills: BTreeSet<ResourceName>
            └── workflows: BTreeSet<ResourceName>

异步读取期间：
AgentImageReadTask
├── reference: AgentImageReference
├── root: Arc<PathBuf>
└── limits: AgentImageLoaderLimits
    -> AgentImageReadOutput
       ├── reference: AgentImageReference
       └── result: Result<PreparedAgentImage, AgentImageLoadError>
           └── PreparedAgentImage
               ├── reference: AgentImageReference
               ├── soul: AgentImageSoul
               ├── model: AgentImageModelConfig
               └── default_visibility: AgentImageDefaultVisibility
```
