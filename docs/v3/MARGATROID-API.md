# Margatroid V3 公开 API

状态：V3 目标契约

本文是 Margatroid 业务 Plugin、CLI/daemon 协议和 Workspace 文件编译器的唯一 API 设计
文档。通用 ECS 与基础设施以 [MECS-API.md](MECS-API.md) 为准；新增业务 API 统一使用
[V3-DESIGN.md 的 API 设计方法论](V3-DESIGN.md#9-api-设计方法论)。

## 1. 总体边界

```text
CLI
├── 读取 margatroid-workspace.yaml
├── 解析项目级 Skill / Workflow
├── 编译 WorkspaceBundle
└── 调用 daemon HTTP API

daemon
├── 管理 AgentImage / Skill / Workflow / Provider
├── 校验并持久化 WorkspaceBundle
├── 管理 Workspace / AgentInstance / Request / Task
└── 运行 ECS 与业务 Plugin
```

CLI 缓存不是权威数据源。daemon 不读取客户端本地路径。协议对象不依赖 ECS、Axum、CLI
或 daemon 实现。

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

WorkspaceId / RequestId / TaskId / AgentId / ResourceId / WorkspaceName
```

ID 在 JSON 中是 string，非空、最长 128 bytes，不允许空白、路径分隔符、`.` 或 `..`。
Plugin Command/Result 暂时使用调用方提供的 `id: String` 配对；出现可查询的远程异步
Operation 后，再把 `OperationId` 作为协议概念加入，当前不提前增加类型。

### 3.2 WorkspaceBundle

```text
WorkspaceBundle
├── schema_version
├── spec: WorkspaceSpec
├── manifest: ResourceManifest
└── resources: BundledResource[]

WorkspaceSpec
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

Skill 和 Workflow 是目录资源包。manifest 固定 kind、逻辑名称、格式版本、media type、大小
和 sha256；正文使用 base64。协议固定 wire shape，不执行文件读取和权威业务校验。

### 3.3 HTTP DTO

```text
CreateWorkspaceRequest / CreateWorkspaceResponse
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
    pub fn bundle(&self) -> &WorkspaceBundle;
    pub fn warnings(&self) -> &[ComposeDiagnostic];
    pub fn into_bundle(self) -> WorkspaceBundle;
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
-> 解析项目级资源
-> 检查 Workflow 依赖
-> 确定性打包
-> 生成 WorkspaceBundle 与 diagnostics
```

相同输入必须生成相同排序、bytes 和 digest。相对路径以 compose 所在项目目录为基准；拒绝
越出项目目录的路径与 symlink。编译器不访问 daemon secret，不修改项目文件，不下载远程资源。

## 7. 当前业务 Plugin 目标 API

### ConfigPlugin

```text
ConfigCommand { Load(path) | Reload }
ConfigResult  { Result<ConfigSnapshot, ConfigError> }
ConfigState   只读当前快照
```

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

Skill 是目录包，不再公开 Markdown 专用的 `SkillDescriptor`、Scan/Load/Unload 八种事件或
`LoadedSkills` 第二份可变注册表。目标 API 是 `SkillCommand`、`SkillResult` 和只读
`SkillCatalog`；具体字段在资源库阶段开始前冻结。

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
    Create(WorkspaceBundle),
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

Catalog、Record、storage snapshot、恢复中间状态和每个动作的独立 Event 都不公开。资源库若
需要被 Workspace 之外的命令独立管理，应成为单独 ResourcePlugin，而不是塞入 Workspace API。

## 8. CLI 命令

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
`workspace up` 默认附着日志，`-d` 后台运行。CLI 可以承担项目解析与本地资源管理，但 daemon
始终拥有运行状态和已安装资源的权威数据。

## 9. 安全与兼容

- Compose、协议、Event、Error 和日志不得包含 Provider secret。
- 所有文件、目录、YAML、bundle、日志和命令输出都有默认上限。
- HTTP DTO 的破坏性变化升级 API major；WorkspaceBundle 破坏性变化升级 schema version。
- Plugin 内部 Event 和 storage 格式不是协议兼容承诺。

## 10. 旧 crate 迁移边界

当前 workspace 中的 `types`、`providers`、`assets`、`paths` 和 `sandbox` 来自旧产品层或被
早期 Plugin 继续依赖。它们不是 V3 facade，不单独承诺稳定 API：

```text
types       仅保留尚被 Provider/LLM 使用的数据，逐步迁入明确领域 crate
providers   收口为 LlmPlugin 的 Provider 扩展边界，不公开 wire 内部模块
assets      由资源库 Plugin 取代
paths       保留内部路径计算，不决定 Workspace 业务语义
sandbox     作为 SandboxPlugin 的平台实现，不作为并列产品入口
```

在替代调用者完成前不机械删除这些 crate；但不得继续向其中添加 V3 Workspace、Agent、Skill
或 Workflow API，也不得从新的统一入口重导出它们。
