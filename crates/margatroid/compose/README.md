# Compose


## 当前存储与 Sidebar

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

新建工作区输入名称与已存在的绝对规范项目目录，提交空成员定义，不创建项目目录或自动启动。新建会话输入名称并显式选择已有有效 AgentDriver（`driver:`，不是 `agent-driver:`）；可用包在启动时扫描，包含尚未挂载的包，没有默认驱动或自动加载语义。

工作区与会话提供重命名/删除菜单。成员删除保留数据库；工作区删除要求已停止且无存活实例/未收口任务，将完整 home 移至内部 `workspaces/.trash/<unique>`，catalog 排除该目录与内部暂存项，不做不可逆数据库删除。

重命名必须保留全部持久 blocks/history 的可访问性、内容和关联。名称目前构成 ResourceID 和数据库命名空间，因此移动目录、改 YAML/ID 及保留 SQLite 文件均不足以证明安全。工作区改名必须停止且无存活实例；运行成员先阻止新受理、等待创建收口并真实 drain/stop，再关闭所有数据库句柄及 blocks/history 视图，才能迁移路径和身份。目标确定保留按名称派生的公开 ResourceID，本范围不引入新的稳定身份模型。重命名必须在全部句柄关闭后，显式将旧 ResourceID 的已知 typed 持久命名空间及当前 history 查询关联迁移到新 ResourceID，保留原始事实性 history/audit 内容；当前迁移实现与对应 scoped 测试覆盖该契约。

所有写入持有完整生命周期 lease、存储锁与 expected revision；新建由存储层检查目标不存在（不是 create 请求字段）。名称冲突/陈旧版本须在更改前拒绝并保持旧数据原样。目录、YAML、多个 SQLite 与 ECS 没有整体原子提交，迁移需有可恢复阶段及明确的部分完成/提交未知结果。

当前网络 payload（外层请求 id 仅作关联）：`workspace.create {name, project_root}`；`workspace.rename {workspace, expected_revision, new_name}`；`workspace.delete {workspace, expected_revision}`；`workspace.member.rename {workspace, agent, expected_revision, new_name}`。workspace/agent 均为规范 ResourceID，成员改名同时核对归属；wire 统一使用 snake_case expected_revision；内部事件的 workspace_id 与 wire workspace 由 DTO 显式转换。create 不接收 revision，存储层以目标不存在为前置条件，不能覆盖已有或保留数据的 home。字段与当前 Protocol/DTO 接线一致；验证结论限于实际执行过的 scoped 与集成场景。

侧栏新建工作区必须使用 backend 目录选择能力取得 daemon 文件系统中的项目目录，再由创建操作重新验证其已存在、绝对且规范；不能以浏览器本地文件选择结果充当 backend 路径。新会话从 `state.sync.drivers` 选择所有已安装且完整验证的规范 Driver ID，包括尚未挂载的包；文件存在或当前已挂载实例列表不足以证明有效。扫描、目录选择、协议投影及持久新增已接通，并有 scoped 与隔离数据浏览器验证；未执行真实模型推理。

当前端点：`workspace.paths.query {path?: String, cursor?: String}` 返回 `workspace.paths {id, path: String, parent: String|null, directories: [{name, path}], next_cursor?: String}`。省略 path 使用 daemon HOME；响应路径均为后端规范目录，只列目录、不列文件、不 mkdir；必须有上限和分页，错误经匹配请求 id 的 `request.failed` 返回。共享 `QueryProjectDirectories`/Result 经注入的 `ProjectDirectoryBrowser` 阻塞能力在异步服务的 blocking worker 执行，DTO 只转换和关联，不做文件 IO。

当前 `driver.catalog.query`/`driver.catalog.refresh` 返回 `driver.catalog {id, drivers: [规范 Driver ID], errors: [{path, message}]}`；启动扫描发布共享 `DriverCatalog`，保留 `state.sync.drivers`。驱动领域校验完整包并报告逐包诊断，不把文件存在视为有效。这些端点及共享浏览能力已实现；驱动有效性限于路径、普通文件、有界 UTF-8、清单 schema/model/dependencies 校验，不编译或执行 Lua。

迁移实施顺序：持有完整生命周期 lease，并按确定顺序同时锁源/目标名称；核对磁盘 revision、目标不存在及全部路径在受控 home 内。工作区改名必须已停止；成员活实例先关闭受理、收口已受理创建并真实 drain/stop，随后关闭同一连接管理器的所有 blocks/history 句柄并处理 WAL/SHM。

Compose 文件存储内部按准备、提交、恢复协调迁移（没有独立注入的公开迁移服务）。内部 `workspaces/.operations/<operation-id>/` 保存 journal、staging 和提交时保留的 backup，不是 registry/index；journal 记录操作类型、源/目标名称、来源 revision、旧→新身份映射及源/暂存校验和，并同步文件和目录。Memory 从关闭句柄的原库通过 SQLite backup 生成一致副本，只在副本事务内更新 `block_definitions.agent`；定义 id、TEMP instance、schema、fields 和 history 的全部列/顺序原样校验保留。当前查询身份由重建的新 Agent/history 句柄投影，不改 history/audit 原始事实。工作区改名覆盖整个 home 的已知成员库，包括已移除但保留的库；成员改名不覆盖保留的同名目标库。未知 schema 或外来 namespace 明确拒绝，提交前保留可恢复原配置/数据。

commit 只发布校验通过的副本，持久写入 `committed` 决策后按 journal 安装目录/成员库与 YAML，保留 backup 并同步目录，以校验和确认安装后写入 `complete`。多 DB、文件、目录与 ECS 不具整体原子性；中断由 recovery 按已确认阶段协调，失败可返回含 `commit_unknown` / `recovery_required` 的诊断，不开空库或自动重启假装回滚。对新身份持久上下文和历史可访问性的验证另由真实 FileStore+Driver 集成测试覆盖。

成员改名的运行策略：若父 Workspace 为 Running 且目标成员有活实例，先真实 drain/stop 并关闭全部数据库视图，再 prepare/commit typed 身份迁移；只有持久改名确认成功且父 Workspace 仍为 Running，才按新名称/新 ID 尝试启动该成员。完整操作 lease 覆盖这次启动结果收口；持久成功后启动失败仍返回新 catalog 条目及已提交 revision，保留改名结果，通过新 ID 的 member_failures、日志和侧栏红点报告运行失败，不回滚名称。原本已停止或无活实例的成员只改持久身份，不自动启动；工作区自身改名始终仅允许停止状态。持久失败不启动新身份，也不能声称旧成员仍在运行；停止成功后失败须报告已停止、当前确认配置/身份及必要 recovery_required 状态。不得在 durable commit 前重启。

文件存储构造及后续受理检查执行内部 recovery；`.operations`、`.trash` 与内部暂存项不参与 catalog 枚举。journal 使用 `preparing`、`prepared`、`committed`、`complete`、`rolled_back` 阶段：未提交且源校验通过时回滚准备阶段，已提交时幂等完成安装。未知、损坏或无法安全恢复的 journal 会设置存储级 `recovery_required`，保守阻止所有 catalog-backed 加载、启动和变更，而非仅封锁源/目标；诊断目录仍可列出。不能静默打开部分新身份或按任意 journal 路径写入。已有阶段中断及重复恢复测试不等于穷举磁盘故障。

## 工作区格式（当前契约）

规范文件位置为 `~/.margatroid/workspaces/<name>/workspace.yaml`，Agent 状态数据库固定为同一工作区 home 下的 `agents/<agent>/state.sqlite`。项目目录不创建 `.margatroid`。

```yaml
name: demo
project_root: /home/example/projects/demo
agents:
  coder:
    driver: driver:local/coder:latest
    home_root: /home/example
```

- `name` 必填，必须与目录名一致，不能为空、不能为 `.` 或 `..`，不能含斜杠、反斜杠或控制字符，最多 128 字节。名称还须满足 `ResourceID` 字段规则。
- `project_root` 必填，必须是已经存在的、绝对且规范化的目录路径；拒绝相对路径、父级遍历与符号链接别名。
- `agents` 必填，可以是名称映射或列表，保留出现顺序；空工作区使用 `agents: []`。列表条目必须有 `name`，映射内部提供的 `name` 必须与键一致。成员名称遵循同样规则，不得重复。
- 顶层不再接受 `manager`。成员不接受 `history_path` 或 `memory_path`，也不接受 `tag`、`image` 和旧资源列表。所有未知字段均被拒绝。
- `driver` 必填，格式为 `driver:scope/name[:tag]`；可省略 `driver:` 前缀，缺省标签规范化为 `latest`。
- `home_root` 可选，默认真实环境变量 `HOME`；显式相对路径以 `project_root` 为基准。它与工作区存储 home 是不同概念。

Workspace 地址为 `workspace:local/<name>:latest`，成员地址为 `agent:<workspace-name>/<agent-name>:latest`。

## 编译 API

```rust,ignore
use compose::{compile, compile_str};
let definition = compile("/home/example/.margatroid/workspaces/demo/workspace.yaml")?;
let definition = compile_str(source, "/home/example/.margatroid/workspaces/demo/workspace.yaml")?;
```

两种入口都要求绝对的 `.../workspaces/<name>/workspace.yaml` 布局；`workspaces` 的祖先可替换为测试临时目录。`compile_str` 的目标文件和工作区目录可以尚不存在。现有数据路径组件不得是符号链接。输入 YAML 上限为 1 MiB，错误消息最多 512 字节。编译不会创建目录、Entity、线程或数据库，不读取驱动正文，也不自动启动工作区。

## 本地 catalog 存储

`FileWorkspaceCatalogStore` 实现共享 `WorkspaceCatalogStore`。daemon 组合根使用 `FileWorkspaceCatalogStore::default_root()` 和 `new(root)` 注入存储，并在启动扫描 catalog。CLI 与前端查询 daemon 的 catalog，不再提交任意本地编译定义或数据库路径。

共享类型在 `margatroid_types::workspace_catalog`：

```text
WorkspaceCatalogEntry { id, name, path, revision, definition: Option<_>, error: Option<String> }
WorkspaceCatalogHandle(Arc<dyn WorkspaceCatalogStore>) // 存储能力 Resource
WorkspaceCatalog(Vec<WorkspaceCatalogEntry>)           // 生命周期刷新后的快照 Resource
```

阻塞存储接口：

```text
list() -> Result<Vec<WorkspaceCatalogEntry>, WorkspaceError>
load(&ResourceID, expected_revision: &str) -> Result<WorkspaceCatalogEntry, WorkspaceError>
save(&WorkspaceDefinition, expected_revision: &str) -> Result<WorkspaceCatalogEntry, WorkspaceError>
```

`revision` 是原始 YAML 字节的 SHA-256，不依赖时间戳。`load` 和 `save` 都核对预期版本；`save` 的空版本只用于尚无配置文件的 home。无效 YAML、名称不匹配、缺失文件和保留了 Agent 数据的工作区目录仍作为带诊断的条目返回，不自动删除或迁移。无可读取源文件的条目版本为空；这些条目不能启动。

同一存储对象使用 Mutex 串行操作，同一 root 的不同进程/对象使用 `.catalog.lock` 文件的 advisory flock。保存先重新编译共享定义并校验身份和派生数据库路径，写入同目录私有临时文件、同步文件、再次比较磁盘源内容、原子重命名并同步父目录。参与存储协议的调用受跨进程锁保护；外部编辑器未参与锁，因此在重命名前再次比较内容以发现已发生的编辑；比较与安装间仍有不合作编辑器竞态，不提供任意外部写者强隔离。

存储锁只覆盖 IO。生命周期服务必须持有整个操作的 lease，覆盖活动状态检查、加载/保存、启动或成员变更；存储自身不判断 Workspace 是否运行。阻塞 IO 必须放在 ECS World 锁外。目录快照由生命周期层刷新，发现配置不代表启动。成员移除不会清理保留的数据库。

## 当前验证范围

Compose/Memory scoped 合计 55 passed，包括 SQLite backup、typed namespace 更新、history 全列与顺序不变、preparing/prepared/committed/complete 中断和重复恢复、内部目录排除。真实 FileStore+Driver 集成 7 项另覆盖运行 sibling、成员改名及停止工作区改名后的持久上下文/历史；这些场景不证明所有磁盘故障或不合作写者竞态均可消除。
