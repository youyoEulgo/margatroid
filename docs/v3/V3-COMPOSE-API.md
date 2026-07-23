# Margatroid V3 Workspace 文件编译器 API

状态：阶段 3 API 设计，尚未实现

目标 crate：`crates/margatroid/compose`（package 由宽泛的 `compose` 改为
`margatroid_compose`）

## 1. 定位与边界

`margatroid_compose` 是纯本地项目编译器，不是 ECS Plugin。它把用户维护的
`margatroid-workspace.yaml` 和项目内 Workspace 资源编译为稳定的 `WorkspaceBundle`：

```text
margatroid-workspace.yaml + local Skill / Workflow resources
→ parse
→ resolve from compose project root
→ validate and normalize
→ hash and package
→ NormalizedProject + WorkspaceBundle
```

负责：

- 解析严格、带版本的 YAML 1.2 Compose 文件。
- 从 Workspace 文件所在目录解析本地 Skill / Workflow 资源，不依赖进程当前工作目录。
- 校验标识符、引用、资源格式、路径边界、大小限制和可选预期 hash。
- 将本地 Workflow 转换为 `Bundled` 引用，将 AgentImage 和其他 daemon 资源转换为稳定引用。
- 生成确定性 `WorkspaceSpec`、`ResourceManifest` 和 `WorkspaceBundle`。
- 为 `margatroid workspace config` 提供不含正文和本地路径的规范化结果。

不负责：

- 启动 ECS App、注册 Plugin、读取或修改 World。
- HTTP 上传、daemon 发现、认证、命令行解析和终端展示。
- 判断 daemon 中的 installed Resource ID 是否真实存在。
- 创建 workspace、执行 workflow、调用 LLM 或持久化状态。
- 从环境变量插值配置或读取 Provider secret。

因此普通 `workspace config/up` 命令保持普通 CLI 流程；未来 `chat/attach/exec -it` 才按需
在命令内部启动临时 mecs App。

## 2. Crate 依赖与模块

目标依赖方向：

```text
margatroid_compose
├── margatroid_protocol
├── serde + YAML 1.2 parser + serde_json
├── sha2 + base64
└── std::fs / std::path
```

禁止依赖：

```text
core_plugin / any mecs crate
apps/cli / apps/daemon
reqwest / Axum / Tokio
legacy types
paths（用户项目路径不属于 daemon 数据目录）
```

设计阶段不固定具体 YAML crate。实现时必须选择仍在维护、能提供位置诊断并允许限制 alias
展开的 YAML 1.2 parser；不能仅因 API 简单而默认采用已停止维护的解析器。

内部模块计划：

```text
src/
├── lib.rs          public API
├── compiler.rs     单一编译流程
├── document.rs     私有 authoring schema
├── diagnostic.rs   稳定诊断类型
├── path.rs         project root 与路径边界
├── resource.rs     类型化资源读取与规范化
├── digest.rs       hash、去重与 manifest
└── render.rs       canonical YAML / JSON 输出
```

原 `roster` 模块不属于项目编译，应在阶段 7 迁入 MemberPlugin 的 prompt/context 组装逻辑。
旧 `types::ComposeFile`、`WorkspaceMeta` 和 `AgentRef` 不再作为新编译器 API。

## 3. margatroid-workspace.yaml v1

默认文件名是 `margatroid-workspace.yaml`，同时兼容 `margatroid-workspace.yml`。`-f` 可以
显式指定任意 YAML 文件路径。默认发现规则为：两个默认文件都不存在时失败；两个同时存在
时报告冲突并要求使用 `-f`，不静默选择其中一个。

## 4. 核心对象模型

Compose 文件不是运行时对象，`ComposeProject` 也不是 Margatroid 概念。它们之间的关系
固定为：

```text
margatroid-workspace.yaml
    ↓ parse / validate
ComposeSpec（临时规范化数据）
    ↓ create / update
Workspace（daemon 持有的持久化运行组）
    ├── AgentInstance
    ├── Workflow 状态
    └── MemoryVolume 引用
```

### 4.1 AgentImage 与 AgentInstance

- `AgentImage` 是 Agent 库中可编辑、可读的人类配置及其静态资源集合。
- `AgentImage` 必须包含独立运行 Agent 所需的 Soul、Skill、Provider 引用、model 和其他
  静态配置；Provider 凭据永远由 daemon 运行时配置提供，不进入镜像。
- `AgentInstance` 是 Workspace 中实际运行的成员。启动时读取一次 AgentImage，形成当前
  镜像 revision 的运行时快照；运行期间不会自动同步 AgentImage 的修改。
- 修改 AgentImage 不影响已有实例。`workspace restart` 才会停止旧实例、读取最新镜像并
  创建新实例；MemoryVolume 保留。
- AgentImage 可以被多个 Workspace 或同一 Workspace 的多个 AgentInstance 使用。

### 4.2 Workspace 与记忆

- `Workspace` 是多个 AgentInstance、Workflow 状态和共享协作数据的生命周期边界。
- Agent 默认记忆是项目级文件，由 Workspace 名称和 Agent 实例 ID 生成稳定路径：
  `{project_root}/.margatroid/workspaces/<workspace_name>/memory/<agent_id>/memory.sql`。
- `MemoryVolume` 是可选的、独立于 AgentImage 和 AgentInstance 的持久化数据。显式绑定
  MemoryVolume 后，实例重建或删除时是否保留 volume 由 Workspace 配置和显式命令决定。
- 默认记忆目录和显式 MemoryVolume 都不属于 AgentImage；修改 AgentImage 不会清空记忆。
- Compose 文件可以被删除，已创建的 Workspace 仍必须能依靠 daemon 持久化数据恢复。

第一版作者格式：

```yaml
schema_version: 1

workspace:
  name: review-team
  description: A deterministic review workspace
  manager: coordinator

agents:
  coordinator:
    image: eulgo/coordinator:v1
    skills:
      - coordinator
    workflows:
      - review
  reviewer:
    image: eulgo/reviewer@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    skills:
      - reviewer

```

### 3.1 顶层规则

- `schema_version` 必填，当前只接受 `1`；它描述 compose 作者格式，不等同于 HTTP API
  version。
- `workspace.name` 可省略。Workspace 名称解析顺序为 CLI `-n` override、compose name、文件
  所在目录名。
- `workspace.manager` 必填，并且必须引用 `agents` 中的一个实例 ID。
- `workspace.manager` 就是用户侧常说的 coordinator 入口角色；这里借鉴 Compose 的紧凑声明风格，
  但不沿用 `ports` 这类网络端口语义。
- `agents.<id>.image` 必填，引用 Agent 库中的 AgentImage；Workspace 文件不内联 AgentImage
  的 Soul、Provider、model 或内建 Skill。
- Agent 默认使用项目级记忆文件
  `{project_root}/.margatroid/workspaces/<workspace_name>/memory/<agent_id>/memory.sql`；
  不需要在 Compose 中声明记忆目录。
- `agents.<id>.memory_volume` 可选，用于显式绑定一个命名 MemoryVolume，覆盖默认记忆文件位置；
  它是 Workspace 编排字段，不是 AgentImage 字段。
- `agents.<id>.skills` 列出该 Agent 在运行时可加载的 Skill。
- `agents.<id>.workflows` 列出分配给该 Agent、可由该 Agent 触发的 Workflow。
- 资源引用可以直接写名字；编译器先按项目 `.margatroid/` 默认目录查找，再允许显式路径覆盖。
- `volumes` 可选，用于声明可被 Agent 显式绑定的命名 MemoryVolume；卷中的内容不属于
  AgentImage。
- 所有 mapping 使用严格字段校验；除 `x-*` 扩展字段外，未知字段和拼写错误必须失败。
- v1 不支持 environment interpolation、include、extends 或 glob。

### 3.2 YAML 语义边界

- 输入按 YAML 1.2 解析，只接受普通 mapping、sequence 和 scalar，不接受多文档输入。
- 为贴近 Docker Compose 的日常编辑体验，支持 anchors、aliases 和 `<<` merge key；解析器
  必须限制 alias 数量、展开深度和展开后的总节点数，防止 alias bomb。
- `x-*` 字段可用于保存作者扩展信息，校验和规范化时忽略，不进入 `WorkspaceBundle`；其他
  unknown field 仍然报错。
- scalar 到目标字段的转换必须严格；例如字符串 `"1"` 不能替代整数 `schema_version: 1`。
- Margatroid Compose 只在文件名、CLI 操作和常用 YAML 编辑体验上对齐 Docker Compose，
  schema 和运行语义并不兼容 Docker Compose。

### 3.3 Workspace 资源声明

技能和工作流通过名字引用：

```text
agents.<id>.skills[]
agents.<id>.workflows[]
volumes.<name>
```

Workspace 文件中的可外部注入资源目前是 Skill 和 Workflow。Skill 通过 `agents.<id>.skills`
声明，Workflow 通过 `agents.<id>.workflows` 声明。每个名字都可以直接写：

```yaml
agents:
  reviewer:
    skills:
      - reviewer
    workflows:
      - review
```

或者：

```yaml
agents:
  reviewer:
    skills:
      - path: skills/reviewer/
```

约束：

- `Skill` 和 `Workflow` 的资源包可以来自项目根目录 `.margatroid/` 下的默认目录，也可以来自
  显式 path。
- 项目级默认目录固定为 `{project_root}/.margatroid/skills/` 和
  `{project_root}/.margatroid/workflows/`。
- 主目录默认目录为 `~/.margatroid/skills/` 和 `~/.margatroid/workflows/`；项目级资源优先于
  主目录资源。
- `path` 用于显式指定来源；`installed` 用于引用 daemon 中已安装的资源。
- AgentImage 内部的 Soul、Provider、model 和内建 Skill 不在此处重复声明。
- 本地 path 可以带 `expected_digest`；它校验换行规范化后的资源 bytes，计算结果不匹配时
  编译失败。
- installed 引用只在本地校验 `ResourceId` 形状，存在性由 daemon 权威校验。
- logical name 在同一个 kind 内唯一；不同 kind 可以使用相同 logical name。
- 同一文件可以被多个 logical name 引用，但 manifest entry 仍分别保留；正文按 digest 去重。
- 资源类型来自声明位置；Workflow 的节点类型由 Workflow schema 和节点注册表解析。

### 3.4 Agent 实例编排

Agent 实例 ID 是 `agents.<id>` mapping 的 key。每个 Agent 只能选择一种形式：

- `image: scope/agent[:tag]`：按逻辑名称和可变 tag 选择 AgentImage。
- `image: scope/agent@sha256:<digest>`：按内容 digest 选择固定 AgentImage。

image 解析、tag 指向的 revision、AgentImage 是否已安装和 Provider 凭据存在性由 daemon
权威校验。Workspace 编译器只校验 image 引用格式，不下载或读取 AgentImage 内容。

## 5. 资源格式

Workspace 编译器 v1 仅打包 UTF-8 Skill / Workflow 资源；AgentImage 不在 Workspace bundle 中展开：

| Kind | 输入 | Manifest media type | v1 预检 |
|---|---|---|---|
| Skill | Skill 资源包目录 | `application/vnd.margatroid.skill` | 包清单、版本和通用 envelope |
| Workflow | Workflow Skill 资源包目录 | `application/vnd.margatroid.workflow` | 包清单、节点 schema 版本和基础引用 |
| AgentImage | Agent 库引用 | 不进入 Workspace manifest | image 引用形状 |
| MemoryVolume | daemon 侧命名卷引用 | 不进入 manifest | volume 名称和绑定策略 |

阶段 3 只验证 Workflow 的文件格式和公共 envelope；DAG、condition、retry 等执行语义由
阶段 6 的 Workflow API 固化后加强。加强校验不得改变 Compose 编译器的公开入口。

AgentImage 的独立格式、构建、版本和发布 API 不属于本 Workspace 编译器，后续由 Agent 库
API 单独设计。Workspace 编译器只输出 image 引用，不复制或修改 AgentImage 内容。

在实现阶段 3 前，`margatroid_protocol` 需要补齐两项共享形状：

- `ResourceManifestEntry.format_version`，表示单个资源内容格式版本。
- 版本化 AgentImage 引用、WorkspaceAgentSpec 和 ResourceReference，供 CLI 编译和 daemon 权威
  解析同一份形状。

目标 `WorkspaceAgentSpec` 形状：

```rust
pub struct WorkspaceAgentSpec {
    pub id: AgentId,
    pub image: AgentImageReference,
    pub skills: Vec<ResourceReference>,
}
```

`AgentImageReference` 可以是带 tag 的逻辑引用，也可以是固定 digest。Provider、Soul、Skill
和 model 都由 daemon 从 AgentImage 中加载，不属于 Workspace 文件的直接字段。

这些是 CLI 与 daemon 的传输契约，应进入 protocol，而不是让 daemon 依赖编译器的私有
authoring schema。协议调整必须同步精确 JSON shape 测试。

## 6. Public API

authoring YAML schema 的 serde struct 第一版保持私有，避免把文件语法和 Rust 构造 API 同时冻结。
稳定公开面只暴露编译入口、结果、限制和诊断：

```rust
pub struct ProjectCompiler;
pub struct CompileOptions;
pub struct ProjectLimits;
pub struct CompileOutput;
pub struct NormalizedProject;
pub struct ComposeDiagnostic;
pub struct ComposeCompileError;
pub enum DiagnosticCode;
pub struct SourceLocation;
pub struct RenderError;

impl ProjectCompiler {
    pub fn new() -> Self;

    pub fn compile(
        &self,
        compose_path: impl AsRef<Path>,
    ) -> Result<CompileOutput, ComposeCompileError>;

    pub fn compile_with_options(
        &self,
        compose_path: impl AsRef<Path>,
        options: &CompileOptions,
    ) -> Result<CompileOutput, ComposeCompileError>;
}

impl CompileOptions {
    pub fn new() -> Self;
    pub fn with_workspace_name(self, name: WorkspaceName) -> Self;
    pub fn with_limits(self, limits: ProjectLimits) -> Self;
}

impl ProjectLimits {
    pub fn default() -> Self;
    pub fn with_max_resource_bytes(self, bytes: u64) -> Self;
    pub fn with_max_bundle_bytes(self, bytes: u64) -> Self;
    pub fn with_max_resources(self, count: usize) -> Self;
    pub fn with_max_yaml_aliases(self, count: usize) -> Self;
    pub fn with_max_yaml_depth(self, depth: usize) -> Self;
    pub fn with_max_yaml_nodes(self, count: usize) -> Self;
}

impl CompileOutput {
    pub fn normalized(&self) -> &NormalizedProject;
    pub fn bundle(&self) -> &WorkspaceBundle;
    pub fn warnings(&self) -> &[ComposeDiagnostic];
    pub fn into_bundle(self) -> WorkspaceBundle;
}

impl NormalizedProject {
    pub fn schema_version(&self) -> SchemaVersion;
    pub fn spec(&self) -> &WorkspaceSpec;
    pub fn manifest(&self) -> &ResourceManifest;
    pub fn to_yaml(&self) -> Result<String, RenderError>;
    pub fn to_json(&self) -> Result<String, RenderError>;
}

impl ComposeCompileError {
    pub fn diagnostics(&self) -> &[ComposeDiagnostic];
}
```

`ProjectCompiler`、`CompileOptions` 和 `ProjectLimits` 均实现 `Default`；普通调用只需
`ProjectCompiler::new().compile(path)`。`compile` 与 `compile_with_options` 必须进入同一个
内部流水线，不能产生两套语义。

默认限制暂定：

```text
single normalized resource  ≤ 1 MiB
decoded bundled contents    ≤ 16 MiB
manifest entries            ≤ 1024
YAML aliases                ≤ 128
YAML expanded depth         ≤ 64
YAML expanded nodes         ≤ 100,000
```

资源大小限制在读取与分配前检查，YAML 结构限制在展开过程中检查；`size_bytes` 和 bundle
上限按 base64 解码前的实际资源 bytes 计算。CLI 后续可以提供更严格的限制，但不能超过
daemon 的权威上限。

## 7. NormalizedProject

`NormalizedProject` 是 `WorkspaceBundle` 去掉 `resources[].content_base64` 后的安全视图：

```text
NormalizedProject
├── schema_version
├── spec: WorkspaceSpec
└── manifest: ResourceManifest
```

它不包含：

- compose 文件绝对路径或本地资源路径。
- 资源正文和 base64。
- CLI cache path。
- API key、token 或环境变量值。

`margatroid workspace config` 必须完成与 `workspace up` 相同的完整编译和预检，然后默认输出
canonical YAML；`--format json` 输出相同对象的 canonical JSON。不能维护一条只解析、不读取
资源的快捷路径，否则 config 与 up 会产生不同结论。

## 8. 确定性

给定相同 compose 与资源 bytes，输出不能受 cwd、目录遍历顺序或 HashMap 随机种子影响：

- project root 固定为 compose 文件父目录的 canonical path。
- 所有相对 path 都相对于 project root，不相对于 shell cwd 或被引用文件。
- 文件通过 canonical path 检查必须仍位于 project root；指向 root 外部的 symlink 拒绝。
- UTF-8 BOM 拒绝，`CRLF` 和 `CR` 规范化为 `LF` 后再计算 hash。
- digest 固定为 `sha256:` 加 64 位小写十六进制。
- manifest 按 `(kind, logical_name, digest)` 排序。
- bundled resources 按 digest 排序，同 digest 正文只存一份。
- agents 按 Agent ID 排序；每个 Agent 的 skills/workflows 按文件中的显式顺序保留。
- base64 使用 RFC 4648 standard alphabet 和 padding。
- 编译产物不写入时间戳、随机 ID、绝对路径或主机信息。

`ProjectCompiler` 不修改项目文件，也不生成 lockfile。未来若增加依赖下载与远端资源，再单独
设计 lockfile；v1 不暗中访问网络。

## 9. 路径与安全

路径检查顺序：

```text
compose path
→ canonical project root
→ join declared relative path
→ reject absolute / parent traversal
→ canonicalize target
→ require target under project root
→ require regular file
→ check metadata size
→ bounded read
→ normalize and hash
```

CLI 的检查用于快速反馈，不是 daemon 的信任边界。daemon 只接收 bytes，不接收本地 path，
并在阶段 4 独立校验 schema、digest、size、media type、引用和自身资源上限。

编译器能保证结构化字段中没有 secret，也不执行环境插值；它无法证明任意 Soul/Skill 正文
没有用户手写的凭据。因此文档和诊断必须明确：资源正文会被上传，不要在项目资源内存放
secret。任何诊断都不得回显资源正文。

## 10. 诊断 API

公开错误不使用 `anyhow::Error`。一次编译尽量收集互不依赖的多个错误：

```rust
pub struct ComposeDiagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub location: Option<SourceLocation>,
}

pub struct SourceLocation {
    pub file: PathBuf,       // 相对 project root
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub field: Option<String>,
}
```

首版稳定 code 至少覆盖：

```text
Io
InvalidYaml
UnknownField
UnsupportedSchema
InvalidIdentifier
MissingManager
DuplicateName
UnknownReference
WrongResourceKind
InvalidPath
PathEscapesProject
InvalidResource
DigestMismatch
ResourceTooLarge
BundleTooLarge
TooManyResources
SecretFieldForbidden
```

`message` 面向人类，调用者只根据 `code` 判断退出类别。路径以 project-relative 形式展示，
错误中不包含资源正文、环境变量值或 secret。

## 11. CLI 接入

阶段 3 的 CLI 调用链：

```text
margatroid workspace config [-f margatroid-workspace.yaml] [-n name] [--format yaml|json]
→ ProjectCompiler::compile_with_options
→ print NormalizedProject
→ exit

margatroid workspace up [-d] [-f margatroid-workspace.yaml] [-n name]
→ ProjectCompiler::compile_with_options
→ POST CreateWorkspaceRequest { bundle }
→ attached log output or detached workspace summary
```

- 未指定 `-f` 时发现当前目录下的 `margatroid-workspace.yaml` 或
  `margatroid-workspace.yml`。两者同时存在则报冲突；`-f` 可以显式指定任意文件路径。
  一旦选定文件，后续编译不再读取 cwd。
- `-n/--name` 只通过 `CompileOptions` 覆盖最终 Workspace 名称，不修改源文件；没有
  `workspace.name` 或 CLI override 时使用 compose 文件所在目录名。
- 编译错误在联网前返回；daemon 错误继续使用 `margatroid_protocol::ErrorCode`。
- Compose 编译逻辑不得复制到 CLI command handler。

Workspace 生命周期命令：

```text
margatroid workspace up [-d] [-f file] [-n name]
margatroid workspace down [-n name]
margatroid workspace stop [-n name]
margatroid workspace start [-n name]
margatroid workspace restart [-n name]
```

目录中存在默认文件时，不需要 `-f`；可以通过 `-n` 覆盖 Workspace 名称。生命周期命令
没有 `-n` 时，使用当前目录 Compose 文件解析出的名称；当前目录没有可发现文件时必须
显式提供 `-n`。`up -n` 仍然需要 Compose 文件，不能只凭名称创建 Workspace。

单 Agent 启动命令：

```text
margatroid run <scope>/<agent>[:tag]
margatroid run <scope>/<agent>@sha256:<digest>
```

`run` 从 Agent 库读取 AgentImage，并创建一个只有一个 AgentInstance 的隐式 Workspace；
Workspace 对用户隐藏，但仍使用相同的生命周期和记忆模型。可选参数：

```text
-n, --name <name>       AgentInstance 名称
-d, --detach            后台运行
-i, --interactive       转发 stdin
-t, --tty               分配交互式终端
--rm                    退出后删除实例，不自动删除 MemoryVolume
-v, --volume <name>     挂载已有 MemoryVolume
--prompt <text>         启动后提交一次 prompt
--env <KEY=VALUE>       设置允许的运行时变量
```

`scope/agent` 是 AgentImage 的逻辑引用，`tag` 用于人类可读的版本选择，digest 用于
确定性选择。Provider 和 model 默认取自镜像；运行时覆盖项必须由后续 Agent API 明确
允许，不能让 CLI 绕过 daemon 的凭据和安全策略。

## 12. 测试契约

实现必须覆盖：

- 同一项目从不同 cwd 和不同相对 compose 路径编译出完全相同的 normalized JSON 与 bundle。
- mapping key 顺序变化不影响排序后产物；每个 Agent 的 workflow 显式顺序保持。
- YAML anchors、aliases、merge key 和 `x-*` 扩展字段遵守既定语义；alias 数量、展开深度和
  总节点限制生效，多文档 YAML 被拒绝。
- 缺失文件、目录代替文件、`..`、绝对路径和越界 symlink。
- unknown field、unsupported schema、重复 logical name、缺失 manager 和错误 kind 引用。
- installed Resource ID 形状校验，但不要求连接 daemon。
- expected digest 匹配与不匹配。
- CRLF/LF 产生相同 digest，正文去重后只保留一份 `BundledResource`。
- 单资源、总 bundle 和资源数量上限在分配前生效。
- normalized 输出和错误不包含绝对路径、正文、API key 或 token。
- `workspace config` 全程不启动 daemon、Tokio 或 ECS App。
- protocol 的 bundle JSON shape 在新增 `format_version` 后同步更新。

## 13. 迁移顺序

阶段 3 实现按以下顺序进行：

1. 先补 protocol 的 manifest format version、AgentImageReference 与 WorkspaceAgentSpec，共享 JSON shape 测试。
2. 将 package `compose` 改名为 `margatroid_compose`，删除对 `types` 和 `paths` 的依赖。
3. 实现私有 authoring schema、诊断、路径边界和资源规范化。
4. 实现共用单一内部流水线的 `compile/compile_with_options` 与确定性 bundle 构建。
5. 删除 compose crate 中的 roster；迁移仍依赖旧 `ComposeFile` 的正式代码。
6. 接入 `margatroid workspace config`，通过离线黑盒测试后再实现 `workspace up` 上传。

本阶段不实现 WorkspacePlugin。daemon 在阶段 4 才接受、权威校验并持久化 bundle。
