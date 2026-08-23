# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# 板块格式

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        图书馆组件与 Plugin
# system     src/system.rs     System 函数
# handler    src/handler.rs    处理函数
# events     src/events.rs     事件类型
# types      src/types.rs      其余类型
# error      src/error.rs      Error 与公开错误分类
```

## lib

lib 只放图书馆组件和 Plugin。

图书馆组件是 Entity 必须挂载的领域组件，组件存在本身表明 Entity 的领域身份；例如：

```text
Agent Entity     必须挂载 Agent 组件 + ResourceId 组件
Workspace Entity 必须挂载 Workspace 组件 + ResourceId 组件
```

AgentImageLoaderPlugin 等 Plugin 结构体，以及 AgentImageLoaderPluginInstalled 等 Plugin 安装标记 Resource，也放在 lib。

## system

system 放 System 函数。System 只负责读取本帧领域事件并克隆，然后调用 handler 中的对应处理函数；System 不展开业务逻辑。

## handler

handler 放处理函数。每个 System 读到的领域事件在 handler 中展开为完整业务逻辑。

## events

events 放事件类型。事件类型只包含字段和 `impl Event`，不实现业务逻辑。

## types

types 放除事件和错误外的其余类型：一次性回执、生命周期状态、组件字段依赖的领域类型、trait 等。

## error

error 放 Error 类型和公开错误分类。

# lib

## 类型

公开：
```text
AgentImage：AgentImage 图书馆组件，公开 Component--AgentImage Entity 必须挂载 AgentImage 和 ResourceId；组件存在本身表明 Entity 是已加载的 AgentImage
    base_driver: AgentImageBaseDriver--已验证的 base.lua 源码程序
    dependencies: AgentImageDependencies--agent.toml 依赖清单
    model: AgentImageModelConfig--中立模型配置
    default_visibility: AgentImageDefaultVisibility--镜像默认可见资源
    base_driver(&self) -> &AgentImageBaseDriver
        读取 Base Driver：公开方法
    dependencies(&self) -> &[AgentImageDependency]
        读取依赖清单：公开方法
    model(&self) -> &AgentImageModelConfig
        读取模型配置：公开方法
    default_visibility(&self) -> impl Iterator<Item = &ResourceId> + '_
        读取默认可见资源：公开方法
    impl MecsComponent for AgentImage

AgentImageLoaderPlugin：AgentImage 加载插件，公开结构体--配置镜像库根目录、加载限制和处理 Schedule
    root: PathBuf--agent-images 根目录，私有
    schedule: String--请求准备、异步响应与 Entity 提交所属 Schedule，私有
    limits: AgentImageLoaderLimits--单个镜像加载限制，私有
    open(root: impl Into<PathBuf>) -> Result<Self, AgentImageLoadError>
        打开插件：公开关联函数，规范化 root 并确保镜像根目录存在
        行为：使用默认限制和 RuntimePlugin::PRE_UPDATE，不扫描或加载具体镜像
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，使用 schedule 替换默认 Schedule 并返回自身
    impl Plugin for AgentImageLoaderPlugin
        Plugin：公开 trait 实现
        build(self, app: &mut App)
            构建插件：安装加载状态、请求准备 System、异步读取处理器和提交 System
            行为：
                确认 RuntimePlugin、AsyncRuntimePlugin 和 ResourceIdPlugin 已安装
                确认 schedule 存在且 AgentImageLoaderPluginInstalled 尚未安装
                插入 AgentImageLoaderPluginInstalled 和 AgentImageLoaderState
                挂载 prepare_agent_image_load_system
                通过 add_async_system 挂载 read_agent_image
                挂载 apply_agent_image_load_system

AgentImageLoaderPluginInstalled：AgentImage 加载插件安装标记，公开单元 Resource--供 WorkspacePlugin 确认依赖并阻止重复安装
    impl Resource for AgentImageLoaderPluginInstalled
```

crate公开：
```text
AgentImageLoaderState：AgentImage 加载状态，crate公开 Resource--保存根目录、加载限制和正在合并的请求
    root: Arc<PathBuf>--agent-images 根目录
    limits: AgentImageLoaderLimits--加载限制
    pending: HashMap<ResourceId, Vec<String>>--同一镜像正在进行的请求 ID
    impl Resource for AgentImageLoaderState
```

# system

## 函数

crate公开：
```text
prepare_agent_image_load_system(world: &mut World)
    准备镜像加载：crate公开 System
    处理事件：LoadAgentImage
    行为：
        克隆本帧全部 LoadAgentImage
        逐个调用 handler::handle_agent_image_load
        不展开业务逻辑

apply_agent_image_load_system(world: &mut World)
    提交镜像：crate公开 System
    处理事件：Result<AgentImageReadOutput, AgentImageTaskError>
    行为：
        克隆本帧全部异步结果
        AgentImageTaskError 只写 system log
        对每个 AgentImageReadOutput 调用 take 取得载荷并逐个调用 apply_agent_image_payload
        不展开业务逻辑
```

# handler

## 函数

crate公开：
```text
handle_agent_image_load(world: &mut World, id: String, reference: ResourceId)
    处理镜像加载请求：crate公开函数，合并同一镜像的并发读取并提交异步任务
    行为：
        id 为空时立即发送 InvalidRequest 结果
        reference 已存在于 pending 时只把 id 追加到等待列表
        reference 没有 pending 时插入包含 id 的等待列表
        克隆 root、limits 和 reference 组成 AgentImageReadTask
        调用 WorldAsyncExt::send_async_event 提交异步读取

read_agent_image(task: AgentImageReadTask) -> Result<AgentImageReadOutput, AgentImageTaskError>
    读取镜像：crate公开异步函数，在 AsyncRuntime 中读取并准备完整 AgentImage
    行为：
        在 panic 捕获边界内调用 read_agent_image_inner
        panic 转换为 TaskPanicked
        Runtime 取消时返回 AgentImageTaskError
        成功或普通失败均包装为 AgentImageReadPayload 和 AgentImageReadOutput

apply_agent_image_payload(world: &mut World, payload: AgentImageReadPayload)
    提交镜像载荷：crate公开函数，取得等待请求并原子选择成功或失败路径
    行为：
        从 pending 移除 payload.reference 并取得全部等待 ID
        失败时为每个等待 ID 发送克隆的 AgentImageLoadError
        成功时复用存活 Entity 或创建新 Entity，插入 ResourceId 和 AgentImage 组件
        为每个等待 ID 发送同一 reference 和 Entity 的 LoadAgentImageResult::Ok

normalize_root(root: PathBuf) -> Result<PathBuf, AgentImageLoadError>
    规范化根：crate公开函数，要求绝对路径、拒绝父级跳转并移除当前目录段

ensure_root(root: &Path) -> Result<(), AgentImageLoadError>
    确保根存在：crate公开函数，创建缺失目录后重新检查最终节点
    行为：最终节点是 symlink 或不是目录时返回 InvalidRoot
```

私有：
```text
read_agent_image_inner(task: AgentImageReadTask) -> Result<PreparedAgentImage, AgentImageLoadError>
    执行镜像读取：私有异步函数，完成目录验证、静态文件读取、清单解析和默认可见性发现
    行为：
        解析镜像目录并验证顶层布局
        有界读取并解析 agent.toml
        schema_version 不是 1 时返回 UnsupportedSchema
        验证 model 非空、无控制字符且不超过读取上限
        解析 dependencies，校验资源 ID 和可选 source 的基本格式
        source 只作为安装提示保留，不在 Loader 阶段下载、复制或解析
        从 base.lua 加载 Base Driver，身份继承 AgentImage 的 scope、name 和 tag 并使用 type=mcl
        验证 prompt 依赖：每个 prompt 依赖按 name 的大写形式在镜像根目录查找 `<NAME>.md` 文件；scope 只允许 system 或 user
        读取前后比较顶层目录与 manifest 文件签名
        从 base.lua 解析默认可见资源
        成功时构造 PreparedAgentImage

validate_model_document(
    document: &AgentImageModelDocument,
    limits: &AgentImageLoaderLimits,
) -> Result<(), AgentImageLoadError>
    验证模型文档：私有函数，只检查模型 ID、停止序列数量和单序列读取上限
    行为：不判断 temperature、top_p、max_output_tokens 或停止序列的业务语义

resolve_image_root(root: &Path, reference: &ResourceId) -> Result<PathBuf, AgentImageLoadError>
    解析镜像目录：私有异步函数，将规范化引用映射到 root/scope/name/tag
    行为：reference 的 type 不是 image 时返回 InvalidResourceName；不存在返回 NotFound；symlink 返回 SymlinkNotAllowed；非目录返回 InvalidLayout

validate_image_layout(root: &Path) -> Result<DirectorySignature, AgentImageLoadError>
    验证镜像布局：私有异步函数，检查 AgentImage 顶层只包含规定文件与目录并返回快照
    行为：
        agent.toml 和 base.lua 必须是普通文件
        任意 `.md` 文件允许存在且必须是普通文件，作为可被依赖清单引用的提示词文档
        skills、hooks、tools 和 shells 缺失时视为空，存在时必须是普通目录
        顶层 symlink 返回 SymlinkNotAllowed；特殊文件和未知入口返回 InvalidLayout

validate_prompt_dependencies(
    image_root: &Path,
    dependencies: &[AgentImageDependency],
) -> Result<(), AgentImageLoadError>
    验证 prompt 依赖：私有函数，检查依赖清单中的 prompt 资源在镜像根目录有对应大写文件，并拒绝重复的 prompt 依赖
    行为：
        prompt 依赖的 scope 必须是 system 或 user
        prompt 依赖按 name 的大写形式在镜像根目录查找 `<NAME>.md` 文件
        相同 prompt scope 和 name（相同 message 类型）重复声明时返回 DuplicateDependency，tag 不同也视为重复
        对应文件不存在时返回 PromptReadFailed

check_directory(path: &Path, root: bool) -> Result<Option<PathBuf>, AgentImageLoadError>
    检查目录：私有异步函数，区分不存在、合法目录与无效镜像来源
    行为：拒绝 symlink 和非目录；root 决定非目录被分类为 InvalidRoot 或 InvalidLayout

directory_signature(path: &Path, maximum_entries: usize) -> Result<DirectorySignature, AgentImageLoadError>
    获取目录签名：私有异步函数，读取并排序直接子项
    行为：拒绝 symlink、特殊文件和超限入口，只在签名中保留入口名称与普通文件或目录类型

read_bounded(
    path: &Path,
    maximum: u64,
    read_error: AgentImageLoadErrorKind,
) -> Result<(Vec<u8>, FileSignature), AgentImageLoadError>
    有界读取：私有异步函数，在读取前后比较文件签名和实际字节数
    行为：最多读取 maximum 加一字节，拒绝超限和读取中变化，成功时返回原始字节与读取前签名

file_signature(path: &Path, read_error: AgentImageLoadErrorKind) -> Result<FileSignature, AgentImageLoadError>
    获取文件签名：私有异步函数，拒绝 symlink 和非普通文件并读取长度与修改时间

has_parent(path: &Path) -> bool
    检查父级跳转：私有函数，返回路径是否包含 ParentDir 组件

parse_default_visibility(
    source: &str,
    dependencies: &[AgentImageDependency],
) -> Result<BTreeSet<ResourceId>, AgentImageLoadError>
    解析默认可见资源：私有函数，从 base.lua 的 IMPORT 别名和 INJECT TO tool_default 行中发现依赖清单内的默认可见资源
    行为：只返回依赖清单中存在的资源 ID；找不到 INJECT 时返回空集合
```

# events

## 类型

公开：
```text
LoadAgentImage：加载 AgentImage，公开事件--请求读取当前磁盘中的逻辑镜像
    id: String--调用方生成的请求 ID，用于配对结果
    reference: ResourceId--type=image 的完整镜像资源 ID
    impl Event for LoadAgentImage

LoadAgentImageResult：加载 AgentImage 结果，公开事件--每个已读取请求对应一个结果
    id: String--原请求 ID
    reference: ResourceId--原镜像资源 ID
    result: Result<Entity, AgentImageLoadError>--成功时返回当前 AgentImage Entity
    impl Event for LoadAgentImageResult
```

crate公开：
```text
AgentImageReadTask：AgentImage 异步读取任务，crate公开事件--不持有 World 引用
    reference: ResourceId--type=image 的目标镜像资源 ID
    root: Arc<PathBuf>--镜像库根目录
    limits: AgentImageLoaderLimits--当前限制快照
    impl Event for AgentImageReadTask
```

# types

## 类型

公开：
```text
AgentImageBaseDriver：AgentImage Base Driver，公开结构体--已经通过大小、UTF-8 和 MCL 加载验证的 base.lua 程序
    program: Arc<MclProgram>--MCL 程序，crate公开字段
    program(&self) -> &Arc<MclProgram>
        读取程序：公开方法

AgentImageBaseMcl：AgentImage Base MCL 别名，公开类型别名--AgentImageBaseDriver 的同义词

AgentImageDependency：AgentImage 依赖项，公开结构体--保存规范化资源 ID 和可选来源
    resource_id: ResourceId--依赖资源 ID，crate公开字段
    source: Option<Arc<str>>--可选来源，crate公开字段
    resource_id(&self) -> &ResourceId
        读取资源 ID：公开方法
    source(&self) -> Option<&str>
        读取来源：公开方法

AgentImageDependencies：AgentImage 依赖清单，公开结构体--保存只读依赖切片
    entries: Arc<[AgentImageDependency]>--依赖项，crate公开字段
    entries(&self) -> &[AgentImageDependency]
        读取依赖项：公开方法

AgentImageModelParameters：AgentImage 模型参数，公开结构体--中立保存 agent.toml 中的可选推理参数
    temperature: Option<f32>--采样温度原始值，crate公开字段
    max_output_tokens: Option<u32>--最大输出 token 数原始值，crate公开字段
    top_p: Option<f32>--核采样参数原始值，crate公开字段
    stop: Arc<[String]>--停止序列原始值，crate公开字段
    temperature(&self) -> Option<f32>
        读取温度：公开方法
    max_output_tokens(&self) -> Option<u32>
        读取输出上限：公开方法
    top_p(&self) -> Option<f32>
        读取核采样参数：公开方法
    stop(&self) -> &[String]
        读取停止序列：公开方法

AgentImageModelConfig：AgentImage 模型配置，公开结构体--中立保存模型 ID 文本和模型参数
    model: Arc<str>--稳定模型 ID 文本，crate公开字段
    parameters: AgentImageModelParameters--模型参数，crate公开字段
    model(&self) -> &str
        读取模型 ID：公开方法
    parameters(&self) -> &AgentImageModelParameters
        读取模型参数：公开方法

AgentImageDefaultVisibility：AgentImage 默认可见资源，公开结构体--保存镜像默认可见的资源 ID 集合
    resources: BTreeSet<ResourceId>--默认可见资源，crate公开字段
    resources(&self) -> impl Iterator<Item = &ResourceId> + '_
        读取默认可见资源：公开方法
```

crate公开：
```text
AgentImageManifest：AgentImage 清单，crate公开结构体--agent.toml 反序列化对象
    schema_version: u32--清单版本，第一版只接受 1
    inference: AgentImageModelDocument--模型配置文档
    dependencies: Vec<AgentImageDependencyDocument>--依赖清单，缺省为空

AgentImageDependencyDocument：依赖清单项，crate公开结构体
    id: String--资源 ID 文本
    source: Option<String>--可选来源

AgentImageModelDocument：AgentImage 模型配置文档，crate公开结构体--只表示 agent.toml 字段
    model: String--稳定模型 ID 文本
    temperature: Option<f32>--可选采样温度
    max_output_tokens: Option<u32>--可选最大输出 token 数
    top_p: Option<f32>--可选核采样参数
    stop: Vec<String>--停止序列，缺省为空

AgentImageLoaderLimits：AgentImage 加载限制，crate公开结构体--限制单个镜像目录的文件数量和内容大小
    max_manifest_bytes: u64--agent.toml 最大字节数
    max_model_id_bytes: usize--模型 ID 最大 UTF-8 字节数
    max_stop_sequences: usize--停止序列数量上限，仅保护资源读取
    max_stop_sequence_bytes: usize--单个停止序列最大 UTF-8 字节数，仅保护资源读取
    impl Default for AgentImageLoaderLimits
        Default：crate公开 trait 实现，使用 64KiB 清单、1KiB 模型 ID、128 个停止序列和 4KiB 单序列限制

PreparedAgentImage：已准备 AgentImage，crate公开结构体--镜像静态数据读取与名称发现均已完成
    reference: ResourceId--type=image 的镜像资源 ID
    base_driver: AgentImageBaseDriver--已验证的 Base Driver
    dependencies: AgentImageDependencies--依赖清单
    model: AgentImageModelConfig--中立模型配置
    default_visibility: AgentImageDefaultVisibility--默认可见资源

AgentImageReadPayload：AgentImage 读取载荷，crate公开结构体--无论成功失败都保留镜像引用
    reference: ResourceId--type=image 的原镜像资源 ID
    result: Result<PreparedAgentImage, AgentImageLoadError>--读取结果

AgentImageReadOutput：AgentImage 读取输出，crate公开结构体--允许提交 System 从共享事件引用中取得一次载荷所有权
    payload: Mutex<Option<AgentImageReadPayload>>--尚未提交的读取载荷，私有
    new(payload: AgentImageReadPayload) -> Self
        构造输出：crate公开关联函数，将 payload 保存为尚未取得的载荷
    take(&self) -> Option<AgentImageReadPayload>
        取得载荷：crate公开方法，从共享事件中取出一次载荷所有权；已取出时返回空

DirectoryEntryKind：目录入口类型，crate公开枚举--目录快照只区分 Loader 允许的普通文件和目录
    File
    Directory

DirectoryEntrySignature：目录入口签名，crate公开结构体--用于发现加载期间的目录结构变化
    name: OsString--入口名称
    kind: DirectoryEntryKind--入口类型

DirectorySignature：目录签名，crate公开结构体--按名称排序的直接子项快照
    entries: Vec<DirectoryEntrySignature>--直接子项

FileSignature：文件签名，crate公开结构体--用于识别静态文件读取期间的可见变化
    length: u64--文件长度
    modified: Option<SystemTime>--文件系统可用时的修改时间
```

# error

## 类型

公开：
```text
AgentImageLoadErrorKind：AgentImage 加载错误分类，公开枚举
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
    PromptReadFailed
    DuplicateDependency
    InvalidResourceName
    BaseMclLoadFailed
    TaskPanicked

AgentImageLoadError：AgentImage 加载错误，公开结构体--提供稳定分类和不暴露绝对路径的安全描述
    kind: AgentImageLoadErrorKind--错误分类，私有
    message: String--有界安全描述，私有
    new(kind: AgentImageLoadErrorKind, message: impl Into<String>) -> Self
        构造错误：crate公开关联函数，保存 kind 和安全描述
        行为：message 超过 512 UTF-8 字节时在字符边界截断并追加省略号
    invalid_root(message: impl Into<String>) -> Self
        构造根错误：crate公开关联函数，以 InvalidRoot 分类调用 new
    kind(&self) -> AgentImageLoadErrorKind
        读取分类：公开方法
    message(&self) -> &str
        读取描述：公开方法
    impl Clone for AgentImageLoadError
        Clone：公开 trait 实现，允许一个合并读取结果回答多个等待者
    impl fmt::Display for AgentImageLoadError
        Display：公开 trait 实现，只输出 kind 和有界 message
    impl std::error::Error for AgentImageLoadError
        Error：公开 trait 实现
```

crate公开：
```text
AgentImageTaskError：AgentImage 异步监督错误，crate公开结构体--表示 AsyncRuntime 整体取消处理器
    source: AsyncTaskError--异步运行时错误，crate公开字段
    impl From<AsyncTaskError> for AgentImageTaskError
        From<AsyncTaskError>：crate公开 trait 实现，满足 add_async_system 错误约束
        from(source: AsyncTaskError) -> Self
            转换监督错误：保存 source
```

# 逻辑

```text
安装：
    AgentImageLoaderPlugin::open(agent_images_root)
        -> 验证并创建根目录
        -> app.add_plugin(agent_image_loader_plugin)
        -> 插入空 AgentImageLoaderState
        -> 挂载请求准备、异步读取和主线程提交 System
        -> 不扫描具体镜像，不创建 AgentImage Entity

Workspace 启动：
    WorkspacePlugin 接收 Compose 产生的 WorkspaceDefinition
        -> 为每个 Agent 发送 LoadAgentImage { id, reference }
        -> Loader 按 reference 合并并发读取
        -> AsyncRuntime 读取和结构验证磁盘镜像
        -> 回到主线程创建或刷新 AgentImage Entity
        -> 发送 LoadAgentImageResult { id, reference, result }

Workspace 重载：
    再次发送 LoadAgentImage
        -> 总是重新读取权威磁盘目录
        -> 全量替换当前 AgentImage Entity 的静态组件
        -> 新 AgentInstance 取得新的静态配置与资源可见性
        -> 旧 AgentInstance 的静态配置保持不变

同一镜像并发加载：
    第一条请求创建 pending 并启动一次异步读取
    后续请求只追加自己的 id
    一次读取完成后全部等待者收到各自 id 对应的相同结果

失败：
    任一文件或结构验证失败
        -> 不创建新 Entity
        -> 不修改已有 Entity
        -> 每个等待者收到 AgentImageLoadError
        -> WorkspacePlugin 终止本次 Workspace 启动或重载
```

# 持有关系

```text
App
└── World
    ├── AgentImageLoaderState Resource
    │   ├── root: Arc<PathBuf>
    │   ├── limits: AgentImageLoaderLimits
    │   └── pending: HashMap<ResourceId, Vec<String>>
    └── AgentImage Entity
        ├── ResourceId
        └── AgentImage
            ├── base_driver: AgentImageBaseDriver
            ├── dependencies: AgentImageDependencies
            ├── model: AgentImageModelConfig
            └── default_visibility: AgentImageDefaultVisibility

异步读取期间：
AgentImageReadTask
├── reference: ResourceId
├── root: Arc<PathBuf>
└── limits: AgentImageLoaderLimits
    -> AgentImageReadOutput
       └── payload: Mutex<Option<AgentImageReadPayload>>
           ├── reference: ResourceId
           └── result: Result<PreparedAgentImage, AgentImageLoadError>
               └── PreparedAgentImage
                   ├── reference: ResourceId
                   ├── base_driver: AgentImageBaseDriver
                   ├── dependencies: AgentImageDependencies
                   ├── model: AgentImageModelConfig
                   └── default_visibility: AgentImageDefaultVisibility
```
