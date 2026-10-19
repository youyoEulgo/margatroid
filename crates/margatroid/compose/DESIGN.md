# Compose 与工作区目录存储


## 当前存储与边界

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

Sidebar 新建工作区接收名称及已存在的绝对规范项目目录，持久化空成员定义；不创建项目目录或自动启动。会话新增选择已安装的有效 AgentDriver，使用 `driver:` 地址；有效包的启动扫描由驱动领域负责，Compose 不引入默认驱动或自动加载。

工作区删除：要求已停止且无存活实例及未收口任务，将完整 home 移至内部 `workspaces/.trash/<unique>`，catalog 排除 `.trash` 与内部暂存项，不递归销毁数据库。成员删除仍是定义移除并保留数据库。

重命名：保留所有持久 blocks/history 的内容、可访问性和关联。工作区改名只在停止且无存活实例时进行；运行成员先阻止新受理、等待已受理创建并真实 drain/stop。全部数据库句柄和同连接管理器的 history/block 视图关闭后，才允许搬迁及迁移身份。当前名称构成 workspace/member ResourceID，ResourceID 参与持久数据库命名空间；仅移动目录、重写 YAML/ResourceID 或让新 ID 打开同一文件不满足数据连续性。

目标确定保留现有按名称派生的公开 ResourceID，本范围不引入新的稳定身份模型。重命名必须在全部数据库句柄关闭后，显式迁移旧 ResourceID 对应的已知 typed 持久命名空间与当前 history 查询关联到新 ResourceID，并保留原始事实性 history/audit 内容；当前由存储内部迁移完成这些操作。复制/移动文件或重写 YAML 本身不足以完成身份迁移。多个数据库事务、配置替换和目录移动没有整体原子性；必须设计可恢复阶段记录，明确报告部分完成/提交未知并重新读取协调，不能承诺单次 rename 原子完成全部操作。

所有写操作受生命周期完整 lease 与存储锁/revision 协议共同保护；新建由存储层检查目标不存在（不是 create 请求字段），重命名保护源和目标名称。名称冲突和首次 revision 冲突在任何副作用前拒绝；停止后再次检测到磁盘修改时报告已停止/仍配置，不能声称运行事实已回滚。

当前网络 payload（外层请求 id 仅作关联）：`workspace.create {name, project_root}`；`workspace.rename {workspace, expected_revision, new_name}`；`workspace.delete {workspace, expected_revision}`；`workspace.member.rename {workspace, agent, expected_revision, new_name}`。workspace/agent 均为规范 ResourceID，成员改名同时核对归属；wire 统一使用 snake_case expected_revision；内部事件的 workspace_id 与 wire workspace 由 DTO 显式转换。create 不接收 revision，存储层以目标不存在为前置条件，不能覆盖已有或保留数据的 home。字段与当前 Protocol/DTO 接线一致；验证结论限于实际执行过的 scoped 与集成场景。

侧栏新建工作区必须使用 backend 目录选择能力取得 daemon 文件系统中的项目目录，再由创建操作重新验证其已存在、绝对且规范；不能以浏览器本地文件选择结果充当 backend 路径。新会话从 `state.sync.drivers` 选择所有已安装且完整验证的规范 Driver ID，包括尚未挂载的包；文件存在或当前已挂载实例列表不足以证明有效。扫描、目录选择、协议投影及持久新增已接通，并有 scoped 与隔离数据浏览器验证；未执行真实模型推理。

当前端点：`workspace.paths.query {path?: String, cursor?: String}` 返回 `workspace.paths {id, path: String, parent: String|null, directories: [{name, path}], next_cursor?: String}`。省略 path 使用 daemon HOME；响应路径均为后端规范目录，只列目录、不列文件、不 mkdir；必须有上限和分页，错误经匹配请求 id 的 `request.failed` 返回。共享 `QueryProjectDirectories`/Result 经注入的 `ProjectDirectoryBrowser` 阻塞能力在异步服务的 blocking worker 执行，DTO 只转换和关联，不做文件 IO。

当前 `driver.catalog.query`/`driver.catalog.refresh` 返回 `driver.catalog {id, drivers: [规范 Driver ID], errors: [{path, message}]}`；启动扫描发布共享 `DriverCatalog`，保留 `state.sync.drivers`。驱动领域校验完整包并报告逐包诊断，不把文件存在视为有效。这些端点及共享浏览能力已实现；驱动有效性限于路径、普通文件、有界 UTF-8、清单 schema/model/dependencies 校验，不编译或执行 Lua。

迁移实施顺序：持有完整生命周期 lease，并按确定顺序同时锁源/目标名称；核对磁盘 revision、目标不存在及全部路径在受控 home 内。工作区改名必须已停止；成员活实例先关闭受理、收口已受理创建并真实 drain/stop，随后关闭同一连接管理器的所有 blocks/history 句柄并处理 WAL/SHM。

Compose 文件存储内部按准备、提交、恢复协调迁移（没有独立注入的公开迁移服务）。内部 `workspaces/.operations/<operation-id>/` 保存 journal、staging 和提交时保留的 backup，不是 registry/index；journal 记录操作类型、源/目标名称、来源 revision、旧→新身份映射及源/暂存校验和，并同步文件和目录。Memory 从关闭句柄的原库通过 SQLite backup 生成一致副本，只在副本事务内更新 `block_definitions.agent`；定义 id、TEMP instance、schema、fields 和 history 的全部列/顺序原样校验保留。当前查询身份由重建的新 Agent/history 句柄投影，不改 history/audit 原始事实。工作区改名覆盖整个 home 的已知成员库，包括已移除但保留的库；成员改名不覆盖保留的同名目标库。未知 schema 或外来 namespace 明确拒绝，提交前保留可恢复原配置/数据。

commit 只发布校验通过的副本，持久写入 `committed` 决策后按 journal 安装目录/成员库与 YAML，保留 backup 并同步目录，以校验和确认安装后写入 `complete`。多 DB、文件、目录与 ECS 不具整体原子性；中断由 recovery 按已确认阶段协调，失败可返回含 `commit_unknown` / `recovery_required` 的诊断，不开空库或自动重启假装回滚。对新身份持久上下文和历史可访问性的验证另由真实 FileStore+Driver 集成测试覆盖。

成员改名的运行策略：若父 Workspace 为 Running 且目标成员有活实例，先真实 drain/stop 并关闭全部数据库视图，再 prepare/commit typed 身份迁移；只有持久改名确认成功且父 Workspace 仍为 Running，才按新名称/新 ID 尝试启动该成员。完整操作 lease 覆盖这次启动结果收口；持久成功后启动失败仍返回新 catalog 条目及已提交 revision，保留改名结果，通过新 ID 的 member_failures、日志和侧栏红点报告运行失败，不回滚名称。原本已停止或无活实例的成员只改持久身份，不自动启动；工作区自身改名始终仅允许停止状态。持久失败不启动新身份，也不能声称旧成员仍在运行；停止成功后失败须报告已停止、当前确认配置/身份及必要 recovery_required 状态。不得在 durable commit 前重启。

文件存储构造及后续受理检查执行内部 recovery；`.operations`、`.trash` 与内部暂存项不参与 catalog 枚举。journal 使用 `preparing`、`prepared`、`committed`、`complete`、`rolled_back` 阶段：未提交且源校验通过时回滚准备阶段，已提交时幂等完成安装。未知、损坏或无法安全恢复的 journal 会设置存储级 `recovery_required`，保守阻止所有 catalog-backed 加载、启动和变更，而非仅封锁源/目标；诊断目录仍可列出。不能静默打开部分新身份或按任意 journal 路径写入。已有阶段中断及重复恢复测试不等于穷举磁盘故障。

## 布局与类型（当前契约）

```text
~/.margatroid/workspaces/<name>/
├── workspace.yaml
└── agents/<agent>/state.sqlite
```

工作区名称与 home 目录一致，项目目录不保存工作区定义或 Agent 数据库。固定文件名为 `workspace.yaml`。测试可以注入临时的 `.../workspaces` root，生产组合根使用 `FileWorkspaceCatalogStore::default_root()`。

`WorkspaceDefinition { id, name, project_root, agents }` 和 `WorkspaceAgentDefinition { id, name, driver, home_root, history_path }` 属于共享类型。`history_path` 是运行期需要的派生值，不是 YAML 输入。静态定义没有 `manager`。

顶层 YAML 只接受 `name`、`project_root` 和 `agents`。项目根必须显式填写已存在的绝对规范目录。成员只接受 `name`、`driver` 和 `home_root`；成员列表与映射均保留原顺序，拒绝重复名称。`home_root` 默认真实 HOME，显式相对路径按项目根解析。输入拒绝未知字段、`history_path`、`memory_path`、独立 tag 以及旧资源列表。

地址分别固定为 `workspace:local/<name>:latest` 和 `agent:<workspace-name>/<agent-name>:latest`。驱动地址接受可省略的 `driver:` 前缀，默认标签为 `latest`。

## 编译职责

`compile(path)` 与 `compile_str(source, workspace_file)` 都验证绝对的 `.../workspaces/<name>/workspace.yaml` 结构和目录名称一致性。后者允许目标文件尚未创建。输入最多 1 MiB，错误消息最多 512 字节，UTF-8 边界截断。文件读取只接受普通文件；Unix 使用 `O_NOFOLLOW | O_NONBLOCK` 防止跟随最终符号链接或阻塞在 FIFO。

已有工作区、配置文件和所配置成员的数据路径组件不能是符号链接。项目根要求与 `canonicalize` 结果相等，防止将别名当作规范目录。编译不创建任何目录、Entity、线程或数据库，不读取资源正文，也不进行自动启动。

## 共享 catalog API

`margatroid_types::workspace_catalog` 定义：

```text
WorkspaceCatalogEntry: Clone + Debug + Serialize + Deserialize
    id: ResourceID
    name: String
    path: PathBuf
    revision: String
    definition: Option<WorkspaceDefinition>
    error: Option<String>

WorkspaceCatalogStore: Send + Sync + 'static
    list(&self) -> Result<Vec<WorkspaceCatalogEntry>, WorkspaceError>
    load(&self, id: &ResourceID, expected_revision: &str)
        -> Result<WorkspaceCatalogEntry, WorkspaceError>
    save(&self, definition: &WorkspaceDefinition, expected_revision: &str)
        -> Result<WorkspaceCatalogEntry, WorkspaceError>

WorkspaceCatalogHandle(Arc<dyn WorkspaceCatalogStore>): Clone + Resource
WorkspaceCatalog(Vec<WorkspaceCatalogEntry>): Clone + Debug + Default + Resource
```

存储能力和可序列化目录快照分离。daemon 启动时扫描；生命周期操作完成后刷新快照。目录发现不启动 Workspace。CLI 和前端只使用 daemon 的发现结果，不维护另一套文件扫描或信任客户端数据库路径。

条目的 `revision` 是精确原始 YAML 字节的 SHA-256；注释变化也会改变版本，修改时间不参与。无法读取源文件时版本为空，条目携带诊断。无效 YAML、缺失文件、保留了 agents 数据但删除了配置的目录均保留，不自动清理或迁移。无法转换为合法资源地址的目录采用带名称哈希的诊断 ID；它们不是可启动定义。

`load` 和 `save` 要求精确版本匹配，冲突返回 `WorkspaceBusy`。空版本保存只适用于缺失源文件。无效条目也可返回给调用者检查，生命周期必须在启动前要求 `definition` 存在且 `error` 为空。其他存储/校验错误归为 `InvalidDefinition`。

## 持久迁移服务边界（当前实现）

当前共享 API 是 `WorkspaceCatalogStore` 的 list/load/save、rename/rename_workspace、rename_member 和 delete/delete_workspace，以及 `WorkspaceDeletion`。具体 prepare/commit/recover 是 Compose 存储内部 journal 操作，不是注入的独立公开迁移 trait。Workspace 通过 catalog handle 委托迁移并协调真实停止与完整 lease；Memory 提供 `prepare_agent_identity_copy`，不负责生命周期。

## 原子保存与并发边界

`FileWorkspaceCatalogStore` 位于 Compose crate 的 `storage` 模块，不新增 crate。其 `new(root)` 安全创建缺失 root；所有阻塞操作使用对象 Mutex，并在 root 的 `.catalog.lock` 上持有标准库文件锁，对同一 root 的不同对象和进程串行化访问。

保存完整步骤：

1. 从共享定义生成没有运行期路径的 YAML，重新编译，并校验工作区及成员身份、派生数据库路径，不静默接受客户端路径覆盖。
2. 读取并核对当前源字节版本，拒绝陈旧请求。
3. 在同一工作区 home 创建私有临时文件，写入并 `sync_all`。
4. 再读取当前源并比较原始字节，拒绝已经发生的外部编辑。
5. 原子安装配置并同步父目录；失败时仅清理本操作创建的临时文件，不清理任何 Agent 状态目录。

参与存储协议的写入由 advisory flock 保护；外部编辑器不遵循该锁时，重命名前再次比较内容提供冲突检测，但比较到安装之间仍存在不合作编辑器竞态，advisory 锁不提供对任意编辑器的强隔离。符号链接检查覆盖所有现存数据路径组件，最终源文件打开额外使用 Unix `O_NOFOLLOW`。

存储锁只保护 IO，不能代替生命周期 lease。生命周期必须在完整操作期间持有自己的工作区操作 lease，覆盖活动状态检查、存储调用、启动/停止和成员变更。阻塞存储调用在 ECS World 锁外执行。成员新增先持久化完整定义再发布或启动；移除不删除已有数据库。生命周期与消息协议归各自服务所有，不由 Compose 实现。

## 验证

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

## 当前验证范围

Compose/Memory scoped 合计 55 passed，包括 SQLite backup、typed namespace 更新、history 全列与顺序不变、preparing/prepared/committed/complete 中断和重复恢复、内部目录排除。真实 FileStore+Driver 集成 7 项另覆盖运行 sibling、成员改名及停止工作区改名后的持久上下文/历史；这些场景不证明所有磁盘故障或不合作写者竞态均可消除。
