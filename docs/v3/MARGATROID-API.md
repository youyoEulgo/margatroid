# Margatroid V3 公开 API

状态：V3 目标契约

本文是 Margatroid 业务 Plugin、CLI/daemon 协议和 Workspace 文件编译器的唯一 API 设计
文档。通用 ECS 与基础设施以 [MECS-API.md](MECS-API.md) 为准；新增业务 API 统一使用
[V3-DESIGN.md 的 API 设计方法论](V3-DESIGN.md#9-api-设计方法论)。

## 1. 总体边界

```text
CLI
├── 查找 margatroid-workspace.yaml
├── 发送路径与控制命令
├── 展示诊断、状态与日志
└── 调用 daemon HTTP API

daemon
├── 读取 compose 与项目级资源
├── 管理主目录 AgentImage / Skill / Workflow / Provider
├── 校验资源并生成不可变运行快照
├── 持续读写项目级 Memory
├── 管理 Workspace / AgentInstance / Request / Task
└── 运行 ECS 与业务 Plugin
```

V3 第一版只支持 CLI 与 daemon 位于同一台机器并共享本地文件系统。CLI 是可随时
退出的控制端，不持有资源副本，不同步 Memory，也不与 Workspace 同生命周期。daemon
是主目录资源、运行状态和 Memory 的唯一运行时 Owner。协议对象不依赖 ECS、Axum、
CLI 或 daemon 实现。

## 2. 业务 Plugin 规则

### 2.1 单写入口

每个可变业务能力只公开一对 Event：

```rust
pub struct XxxCommand {
    pub id: String,
    pub action: XxxAction,
}

pub struct XxxResult {
    pub id: String,
    pub result: Result<XxxChange, XxxError>,
}
```

- Command 是唯一写入口。
- Resource 只公开查询快照，不公开等价的 set/load/delete 方法。
- Started/Progress/Chunk 仅在存在独立消费者时单独成为 Event。
- 同步操作也使用同一形状，调用者不依赖“当前恰好同步”的实现事实。
- 一个 Plugin 只公开一个领域 Error；Error 内使用 kind 表达分类。

### 2.2 公共类型分类

Plugin 的公开类型只能是：Plugin、Options、Command、Result、只读查询 Resource、必要的
Component 或扩展 Trait。worker、channel、storage、解析中间对象和内部 Event 不公开。

## 3. 产品协议

实现 crate：`margatroid_protocol`。只依赖 serde。

### 3.1 版本与 ID

```text
API_VERSION = "v1"
CURRENT_SCHEMA_VERSION = 1

WorkspaceId / RequestId / TaskId / AgentId / ResourceId / ResourceName / WorkspaceName
```

ID 在 JSON 中是 string，非空、最长 128 bytes，不允许空白、路径分隔符、`.` 或 `..`。
Plugin Command/Result 暂时使用调用方提供的 `id: String` 配对；真正出现可查询的异步
Operation 后，再把 `OperationId` 作为协议概念加入，当前不提前增加类型。

### 3.2 WorkspaceSpec

```text
WorkspaceSpec
├── schema_version
├── name
├── description?
├── manager: AgentId
└── agents: WorkspaceAgentSpec[]

WorkspaceAgentSpec
├── id
├── image: AgentImageReference
├── skills: ResourceReference[]
├── workflows: ResourceReference[]
└── memory_volume?
```

HTTP `workspace up` 传递 daemon 可见的 compose 路径，daemon 使用 Compose 编译器
生成 `WorkspaceSpec` 和规范化项目根目录。协议不携带 Skill、Workflow、AgentImage 或
Memory 正文；它们均由 daemon 从共享文件系统读取。CLI 先将用户输入相对自己的
当前目录转为绝对路径；daemon 拒绝相对路径并再次规范化，不使用自己的工作目录猜测。

### 3.3 HTTP DTO

```text
CreateWorkspaceRequest { compose_path: String } / CreateWorkspaceResponse
ListWorkspacesResponse / WorkspaceSummary
SubmitPromptRequest / SubmitPromptResponse
GetRequestResponse / RequestSummary / TaskSummary / TaskResult
ErrorResponse
```

薄 Request/Response 结构是 HTTP 版本边界，可以保留。协议层不复用 Plugin Command Event。

## 4. Agent 与 Workspace

- AgentImage 是可独立启动 Agent 所需静态资源的集合，包含 soul、provider/model 引用以及
  镜像内 Skill / Workflow。
- AgentImage 可以被人修改，但运行中的 AgentInstance 不热更新；重启后重新读取。
- AgentInstance 是 Workspace 中由 AgentImage 启动出的运行对象。
- Workspace 是一组 AgentInstance 的运行组，不是 Compose 文件本身。
- 同一 Agent 在不同 Workspace 中的记忆完全隔离。

默认目录：

```text
~/.margatroid/                         主目录
project/.margatroid/                  项目级目录
project/.margatroid/workspaces/demo/memory/coder/memory.sql
project/.margatroid/skills/<scope>/<name>/
project/.margatroid/workflows/<scope>/<name>/
```

资源解析优先级为 AgentImage 内置 > 项目级 > 主目录。同名资源由更窄作用域覆盖；Compose
仍必须显式声明该 Agent 常态可见的额外 Skill / Workflow。

## 5. Workspace 文件

默认文件名为 `margatroid-workspace.yaml`，兼容 `.yml`。顶层只包含 schema 版本、Workspace
元数据、agents 和可选具名记忆卷；没有全局 `resources` 字段。

```yaml
schema_version: 1

workspace:
  name: demo
  description: optional
  manager: manager

agents:
  manager:
    image: local/manager:latest
    skills:
      - local/project-context
    workflows:
      - local/review
  coder:
    image: local/coder:latest

volumes:
  shared-memory: {}
```

Agent 的 soul、provider 和 model 属于 AgentImage，不能在 Workspace 文件中重定义。未指定
memory 时自动使用项目级
`.margatroid/workspaces/<workspace-name>/memory/<agent-id>/memory.sql`；`memory_volume` 和顶层
`volumes` 只用于显式覆盖默认位置。`workspace.manager` 必须引用 agents 中的普通 Agent，
它就是用户语境中的 coordinator，不是特殊身份，也不是网络端口桥接。

Workflow 与 Skill 一样是带作用域的目录包，可以包含脚本、提示词模板和依赖清单。Workflow
属于具体 Agent，不在 Workspace 顶层定义。依赖检查范围是镜像内、项目级和主目录 Skill。

## 6. Compose 编译器

实现 crate：`margatroid_compose`。authoring YAML 的 serde 类型保持私有。

### 6.1 公开 API

```rust
pub fn compile(path: impl AsRef<Path>)
    -> Result<CompileOutput, ComposeCompileError>;

pub struct Compiler;
impl Compiler {
    pub fn new(options: CompileOptions) -> Self;
    pub fn compile(&self, path: impl AsRef<Path>)
        -> Result<CompileOutput, ComposeCompileError>;
}

pub struct CompileOutput;
impl CompileOutput {
    pub fn normalized(&self) -> &NormalizedProject;
    pub fn project_root(&self) -> &Path;
    pub fn workspace(&self) -> &WorkspaceSpec;
    pub fn warnings(&self) -> &[ComposeDiagnostic];
}
```

`compile(path)` 使用默认 Options。高级用户配置一次 Compiler，不提供
`new + Default + compile + compile_with_options` 四套交叉入口。`NormalizedProject` 是不含
资源正文的安全输出视图，供 `workspace config` 渲染 YAML 或 JSON。

### 6.2 编译规则

```text
定位 compose 文件
-> 有界 YAML 解析
-> 校验字段与作用域名称
-> 规范化项目根目录与资源引用
-> 生成 WorkspaceSpec 与 diagnostics
```

相对路径以 compose 所在项目目录为基准。编译器只处理 YAML 结构和引用，不打包
资源正文，不访问 daemon secret，不修改项目文件。资源目录边界、symlink、大小、内容
和 Workflow 依赖由 daemon 在 Workspace 启动快照阶段权威校验。

## 7. ResourceLibraryPlugin API

状态：阶段 4 设计草案，尚未实现；通过 API 审查后再冻结公开面。

### 7.1 目标与非目标

目标：由 daemon 持有主目录 AgentImage、Skill、Workflow 的权威资源库，提供安装、原子更新、
安全删除和只读查询。资源库条目允许人工编辑；Workspace 或 Agent 启动时从当前内容生成不可变
运行快照，运行中的 AgentInstance 不继续读取可变目录。

第一版非目标：

- 不管理 Provider 配置或 secret，它们属于 LlmPlugin。
- 不管理 Workspace 生命周期、Memory 或 AgentInstance。
- 不把项目级 Skill / Workflow 自动安装为主目录共享资源。
- 不下载远程 registry，不提供 publish、pull、push 或自动更新。
- 不公开内部索引 Record、文件锁、事务或垃圾回收 API。
- 不在本阶段解释 Skill / Workflow 内容或验证 Provider 是否存在；资源库只验证安全、确定的
  目录结构，领域语义由后续消费 Plugin 验证。

### 7.2 三个首版场景

普通场景：CLI 将 `local/reviewer` Skill 的本地目录路径发给 daemon，HTTP adapter 转换后发送
Install Command；ResourceLibraryPlugin 自行读取、校验并原子复制目录。调用者
收到 ResourceResult，并能从 ResourceCatalog 按名称查到同一个资源条目和 digest。

```rust
let plugin = ResourceLibraryPlugin::open(data_dir)?;
app.add_plugins(plugin);
app.world().send_event(ResourceCommand {
    id: "install-1".into(),
    action: ResourceAction::InstallSkill {
        name: ResourceName::new("local/reviewer")?,
        source: PathBuf::from("/home/user/reviewer"),
    },
});
```

高级场景：用户修改主目录中 `local/coder:latest` 的人类可读文件，然后重启 Workspace。资源库
重新验证当前目录并生成新 digest；已经启动的 AgentInstance 在停止前继续使用旧快照，重启后的
实例使用新快照。digest 是 daemon 记录的完整性与运行快照元数据，不是用户必须管理的引用语法。

```rust
let before = catalog.image(&tagged_reference)?.unwrap();
atomic_replace_agent_files(&tagged_reference)?;
let after = catalog.image(&tagged_reference)?.unwrap();
assert_eq!(before.id, after.id);
assert_ne!(before.digest, after.digest);
```

失败场景：安装源目录不存在、越界、包含 symlink 或超出上限时，Result 返回确定错误，
主目录中的原资源和索引均保持不变。

```rust
assert_eq!(result.result.unwrap_err().kind(), ResourceErrorKind::InvalidSource);
```

### 7.3 Owner 与边界

```text
Owner               ResourceLibraryPlugin
权威状态             daemon 主目录中的可编辑资源条目
写入者               ResourceCommand consumer system；用户可直接编辑公开资源目录
只读者               WorkspacePlugin、ServerPlugin、Agent runtime
生命周期             daemon 数据生命周期，跨进程重启
线程边界             Command 在 ECS 主线程排序；内部持久化策略不进入 API
信任边界             CLI/HTTP 输入不可信，daemon 校验源路径、大小和目录结构
```

CLI 不读取或打包资源正文。安装命令中的源路径只是 daemon 本次原子复制的输入，成功后
不再是资源的一部分。主目录是允许用户直接编辑的本地信任边界；所有人工修改在下次
查询或启动快照时重新校验，不能绕过包结构与安全限制。直接修改文件不会热更新运行实例，也不具备
ResourceCommand 的原子写入和占用保护；直接删除一个正在使用的目录无法被 daemon 拦截，但运行
快照仍然有效，下一次启动会明确失败。因此正常删除应使用 Command/CLI。

公开且可编辑的主目录布局是：

```text
~/.margatroid/
├── agent-images/<scope>/<name>/<tag>/
├── skills/<scope>/<name>/
└── workflows/<scope>/<name>/
```

自定义 daemon data directory 时只替换 `~/.margatroid` 根。包内文件保持人类可读；内部索引、
临时文件与占用记录的路径和格式不是公开 API。用户直接编辑时应使用“写临时文件再 rename”的
方式；读取过程中出现的半成品按 InvalidResource 处理，不回退到旧内容。

AgentImage 内置 Skill / Workflow 是镜像快照的一部分，不自动成为资源库中的独立条目。
项目级 Skill / Workflow 由 WorkspacePlugin 在启动时从项目根目录解析并拍摄快照，不污染主目录
逻辑名称，也不进入 ResourceCatalog。

### 7.4 唯一主路径

```text
ResourceCommand
-> ResourceLibraryPlugin
-> validate source directory + atomic persistence
-> ResourceCatalog snapshot
-> ResourceResult
```

HTTP 数据流是：

```text
HTTP Request DTO -> ServerPlugin -> ResourceCommand -> ResourceResult -> HTTP Response DTO
```

ServerPlugin 不直接写文件，ResourceCatalog 不提供 install/remove，ResourceLibraryPlugin 不公开
等价 Handle。安装或删除成功的事实已经包含在 ResourceResult 中，第一版不增加
ResourceInstalled / ResourceRemoved Event。

### 7.5 资源标识与运行快照

协议中的 `ResourceKind` 收敛为：

```rust
pub enum ResourceKind {
    AgentImage,
    Skill,
    Workflow,
}
```

`Soul` 是 AgentImage 内容，不是独立资源；`Provider` 由 LlmPlugin 拥有。现有代码中的
`Agent / Soul / Provider` 旧 variant 必须在实现阶段迁移，不能进入资源库 API。

新增协议值对象 `ResourceName`，只接受 `scope/name`。AgentImage 使用现有
`AgentImageReference`：安装与按标签删除只接受 `scope/name[:tag]`，省略 tag 规范化为
`latest`。第一版不支持 `@sha256:<digest>` 引用或历史 revision；Skill / Workflow 第一版没有
tag，同名目录就是同一个可编辑资源条目。

每个资源条目有稳定 `ResourceId`；内容变化时 ID 不变，由 daemon 重算的 `ContentDigest` 改变。
Workspace/Agent 启动时记录实际使用的 ID、digest 和内容快照。运行快照由 Workspace 生命周期
拥有并负责恢复，不进入 ResourceCatalog，也不产生公开 ResourceRevision 类型。人工新增目录首次
被发现时由资源库分配并持久化 ID；这只是从权威文件系统派生索引，不构成第二个业务写入口。人工
重命名目录视为删除旧资源并新增资源，不继承旧 ID。

### 7.6 最小公开类型

```rust
pub struct ResourceLibraryPlugin;

impl ResourceLibraryPlugin {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ResourceError>;
}

pub struct ResourceCommand {
    pub id: String,
    pub action: ResourceAction,
}

pub enum ResourceAction {
    InstallAgentImage {
        reference: AgentImageReference,
        source: PathBuf,
    },
    InstallSkill {
        name: ResourceName,
        source: PathBuf,
    },
    InstallWorkflow {
        name: ResourceName,
        source: PathBuf,
    },
    RemoveAgentImage(AgentImageReference),
    RemoveSkill(ResourceName),
    RemoveWorkflow(ResourceName),
}

pub struct ResourceResult {
    pub id: String,
    pub result: Result<ResourceSummary, ResourceError>,
}

pub struct ResourceCatalog;

impl ResourceCatalog {
    pub fn get(&self, id: &ResourceId) -> Result<Option<ResourceSummary>, ResourceError>;
    pub fn image(
        &self,
        reference: &AgentImageReference,
    ) -> Result<Option<ResourceSummary>, ResourceError>;
    pub fn skill(&self, name: &ResourceName) -> Result<Option<ResourceSummary>, ResourceError>;
    pub fn workflow(&self, name: &ResourceName) -> Result<Option<ResourceSummary>, ResourceError>;
    pub fn list(&self, kind: ResourceKind) -> Result<Vec<ResourceSummary>, ResourceError>;
}

pub struct ResourceSummary {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub name: ResourceName,
    pub tag: Option<String>,
    pub digest: ContentDigest,
    pub size_bytes: u64,
}

pub struct ResourceError;

impl ResourceError {
    pub fn kind(&self) -> ResourceErrorKind;
    pub fn message(&self) -> &str;
}

pub enum ResourceErrorKind {
    InvalidReference,
    InvalidSource,
    InvalidResource,
    LimitExceeded,
    NotFound,
    InUse,
    StoreUnavailable,
    StoreCorrupt,
}
```

分类与公开理由：

| 类型 | 分类 | 首个调用者与公开原因 |
|---|---|---|
| ResourceLibraryPlugin | Plugin | daemon 组合根需要安装能力并显式打开权威目录 |
| ResourceCommand / ResourceAction | Command | ServerPlugin 管理主目录资源的唯一业务写入口 |
| ResourceResult | Result | adapter 需要按 id 关联一次操作的确定结果 |
| ResourceCatalog | Query Resource | WorkspacePlugin、ServerPlugin 需要只读解析和列表 |
| ResourceSummary | Query snapshot | CLI 列表、inspect 和 Result 需要安全稳定的资源视图 |
| ResourceError / ResourceErrorKind | Error | CLI/HTTP 需要稳定分支和安全展示信息 |

Install Action 中的 `source: PathBuf` 是 daemon 可见的一次性本地输入，不是持久化模型。
ServerPlugin 把 HTTP 路径字符串转为平台路径，ResourceLibraryPlugin 负责规范化、安全遍历和复制；
路径必须是由 CLI 根据自己当前目录解析的绝对路径，daemon 不信任 CLI 对路径或内容的判断。
`ResourceCatalog` 每次读取都重新验证可能被人工修改的条目，返回拥有所有权的摘要，不返回锁
guard、Record 引用或可变对象；单个损坏条目不会被静默忽略。列表按逻辑名称、tag、digest
确定性排序。

### 7.7 成功、失败与并发语义

- 每个被 Plugin 读取并接受的 Command 恰好产生一个同 id Result；Result 只在原子提交完成后
  报告成功。关闭后未被读取的输入由 adapter 作为 Unavailable 拒绝，不伪造成功 Result。
- 同一逻辑名称重复安装相同内容是幂等成功，返回当前 ResourceSummary。
- 同一逻辑名称安装不同内容会原子替换可编辑条目，保留 ResourceId 并由 daemon 更新 digest。
- 人工修改会在查询或启动快照时重新校验；读取到中间状态或损坏资源时失败，
  不返回上一份内容冒充成功。
- 删除不存在的逻辑名称返回 NotFound；删除被占用的资源返回 InUse，不做部分删除。
- InvalidReference、InvalidSource、InvalidResource、LimitExceeded 需要调用者修正输入，不应重试。
- InUse 需要先解除 Workspace 占用；StoreUnavailable 可以稍后重试；StoreCorrupt 阻止资源库打开，
  不能跳过损坏条目后部分启动。
- Command 在 ECS 主线程按读取顺序串行提交。同一名称的并发更新以后提交成功者为当前内容，
  每个 Result 都准确对应其提交后的 digest。
- 第一版操作均为本地、有界事务，不提供 cancel/progress。内部以后改为异步 I/O 时，公开 API
  仍保持 Command -> Result。

### 7.8 关闭、恢复与占用关系

`ResourceLibraryPlugin::open` 在根目录不存在时创建空目录，并在加入 App 前验证内部索引和
持久化格式；索引损坏直接返回 StoreCorrupt，不允许部分打开。某个用户资源损坏不阻止 daemon
打开，但查询或启动该资源会
返回 InvalidResource。由此不需要额外 Ready 状态或 ResourceLibraryStarted Event。关闭时不再
接受新 Command，正在提交的事务要么完整落盘，要么保持旧索引；临时文件在下次 open 时确定性
清理或报告损坏。

Workspace 对资源条目和实际快照 digest 的占用登记只有 WorkspacePlugin 存在后才有真实调用者。
该协作接口在阶段 5 按 Workspace 场景设计，不在本阶段提前公开 retain/release/lease Handle，
也不允许给 ResourceCatalog 添加隐藏写方法。阶段 5 接入后，Remove 必须遵守这里定义的 InUse
语义。

### 7.9 暂缓能力与删减结果

暂缓远程 daemon、远程 registry、资源上传与同步、force remove、tag 管理命令、公开 garbage collection、
资源热更新通知、
逐文件读取和资源编辑 API。当前三个场景不需要 Options、Handle、ResourceRecord、
ResourceInstalled Event、ResourceRemoved Event 或可写 Registry，因此这些类型不公开。
资源根目录是创建 Plugin 不可缺少的参数，所以只提供 `open(root)`，不再提供会猜测路径的
`new/default`；官方 daemon 组合负责传入自己的 data directory。

## 8. 后续业务 Plugin 目标 API

### LlmPlugin

```text
LlmCommand
LlmResult
LlmStreamChunk        可选过程 Event
LlmProviders          只读 Provider 查询
```

### SandboxPlugin

```text
SandboxCommand
SandboxResult
SandboxPolicy         只读生效策略
```

### SkillPlugin

Skill 的安装、更新、删除和查询已经属于 ResourceLibraryPlugin。未来 SkillPlugin 只负责把
已解析的目录包接入 Agent 执行，不再拥有第二份 SkillCatalog，也不公开 Markdown 专用的
`SkillDescriptor` 或 Scan/Load/Unload 八种生命周期 Event；执行 API 在阶段 7 按真实调用场景
设计。

### EventBusPlugin

它只负责把产品事件广播给 HTTP/SSE/WebSocket 消费者。发布入口是 ECS 产品 Event；公开
Handle 只允许 subscribe，不允许再直接 publish 同一事件。

### ServerPlugin

只注册 Margatroid HTTP endpoint，把 DTO 转成业务 Command，并把查询状态转成 DTO。监听器、
日志流和 shutdown 由 mecs Plugin 提供。

### DaemonLifecyclePlugin

只公开 `DaemonState` 和只读 `DaemonLifecycle`：

```text
Starting -> Ready -> Draining -> Stopped
```

Daemon 生命周期与 Workspace 生命周期相互独立。

### WorkspacePlugin

```rust
pub struct WorkspacePlugin;
pub struct WorkspaceOptions;
pub struct WorkspaceCommand {
    pub id: String,
    pub action: WorkspaceAction,
}
pub enum WorkspaceAction {
    Up(PathBuf),
    Start(WorkspaceId),
    Stop(WorkspaceId),
    Delete(WorkspaceId),
}
pub struct WorkspaceResult {
    pub id: String,
    pub result: Result<WorkspaceChange, WorkspaceError>,
}
pub struct Workspaces;              // 只读查询
pub struct WorkspaceAgentInstance; // 下游 Plugin 需要的 Component
```

Catalog、Record、storage snapshot、恢复中间状态和每个动作的独立 Event 都不公开。资源库由
独立 ResourceLibraryPlugin 管理，不塞入 Workspace API。
`Up` 接收 daemon 可见的 compose 路径，WorkspacePlugin 调用 Compose 编译器后解析资源并
创建或更新同名 Workspace。ServerPlugin 不编译 compose，只负责将 HTTP 路径字符串转为
`PathBuf` 并发送 Command。

## 9. CLI 命令

```text
margatroid workspace up [-f FILE]
margatroid workspace down [-n NAME]
margatroid workspace stop [-n NAME]
margatroid workspace start [-n NAME]
margatroid workspace restart [-n NAME]
margatroid workspace ps

margatroid run SCOPE/AGENT[:TAG]
margatroid agent ls
margatroid skill ls
margatroid workflow ls
```

未指定文件或名称时，从当前目录向上查找 `margatroid-workspace.yaml` 并使用其中的 name。
`workspace up` 默认附着日志，`-d` 后台运行。CLI 只把找到的 compose 路径发给 daemon；
daemon 编译文件、解析项目资源并创建 Workspace。CLI 等待启动结果或附着日志不代表它
拥有 Workspace；CLI 退出后 daemon 仍持续运行。

## 10. 安全与兼容

- Compose、协议、Event、Error 和日志不得包含 Provider secret。
- 所有文件、目录、YAML、日志和命令输出都有默认上限。
- 路径型 HTTP 命令只对本机 CLI 开放；daemon 不因共享文件系统而信任 CLI 提供的路径。
- HTTP DTO 的破坏性变化升级 API major；Workspace 文件破坏性变化升级 schema version。
- Plugin 内部 Event 和 storage 格式不是协议兼容承诺。

## 11. 旧 crate 边界

早期 Config、EventBus、LLM、Sandbox、Skill Plugin 以及 `types`、`providers`、`assets`、
`sandbox` 已移入 `legacy/prototypes`，退出正式 workspace。它们不是 V3 facade，也不构成
兼容承诺：

```text
types       旧 LLM wire、配置、Member、Compose 与消息类型
providers   旧 Provider client、wire module 与构造逻辑
assets      旧 Member/Workspace 文件资源管理
sandbox     旧平台命令包装与策略实现
```

正式 workspace 中的 `paths` 只负责当前 daemon 主目录、配置文件和 lock 文件路径，不决定
Workspace、资源库或记忆的业务布局。新能力可以参考 legacy 行为，但不得重新依赖其中的 crate。
