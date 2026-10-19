# WorkspacePlugin

`WorkspacePlugin` 协调持久工作区 catalog、可变成员定义及运行实例生命周期。共享 types 提供契约，daemon 注入存储 backend；运行 Entity 与 catalog 条目分离。恢复的 catalog 基线与 CRUD 生命周期协调现已通过本 crate 单元测试；存储迁移、驱动发现、目录选择与 UI 由各自 scoped/集成场景独立验证，不由这 19 项测试单独证明。

2026-10-18 恢复验证：`cargo test -p workspace_plugin --lib` **19 passed**（原有 15 项，加 4 项 CRUD 回归）。新增覆盖空工作区创建及防覆盖、停止工作区改名后可恢复删除、活成员真实 StopAgent 结果后才调用 durable rename 并经 LoadAgentDriver/StartAgent 重启、重启失败保留新定义，以及原已停止成员不自动启动和停止后持久失败的诚实诊断。CRUD 测试使用 typed store 替身，不证明 SQLite 迁移、journal/recover 或文件系统故障注入已验收。

## Sidebar 与生命周期

新建工作区输入名称与已存在的绝对、规范项目路径，提交空成员定义，不创建项目目录或自动启动。新建会话输入名称并显式选择已有 AgentDriver，地址为 `driver:scope/name:tag`，不使用 `agent-driver:`。启动时扫描有效已安装驱动包，包含尚未挂载的包；没有默认驱动或自动加载语义。当前扫描读取并校验有界 UTF-8 与清单；不编译或执行 Lua，也不保证依赖或模型运行可用。

工作区与会话菜单提供重命名和删除。会话删除沿用成员移除并保留数据库；工作区删除要求已停止且无存活实例，完整 home 移到内部 `workspaces/.trash/<unique>`，catalog 排除该内部目录，不进行不可逆数据库删除。

重命名必须保留全部持久 blocks/history 的内容、可访问性与关联。名称目前参与 ResourceID 及数据库命名空间，移动文件或更换 ID 本身不足以恢复旧数据。工作区改名要求已停止且无存活实例；运行成员改名先阻止新受理、等待已受理创建、真实 drain/stop，再关闭所有数据库句柄及 blocks/history 视图，才允许身份和路径迁移。目标确定保留按名称派生的公开 ResourceID，本范围不引入新的稳定身份模型。重命名必须在全部句柄关闭后，将旧 ResourceID 的已知 typed 持久命名空间及当前 history 查询关联显式迁移到新 ResourceID，并保留原始事实性 history/audit 内容；本 crate 将持久身份迁移委托 typed store；单元测试只验证调用顺序，不证明存储迁移契约已验收。

所有写入经过完整 lease 与 revision 检查；新建由存储层检查目标必须不存在（create 请求不带 revision）。名称冲突在写入前拒绝，旧数据保持原样。多个数据库、目录、YAML 与 ECS 不具整体原子性；迁移必须可恢复并明确报告部分完成或提交未知。

当前网络 payload（外层请求 id 仅作关联）：`workspace.create {name, project_root}`；`workspace.rename {workspace, expected_revision, new_name}`；`workspace.delete {workspace, expected_revision}`；`workspace.member.rename {workspace, agent, expected_revision, new_name}`。workspace/agent 均为规范 ResourceID，成员改名同时核对归属；wire 统一使用 snake_case expected_revision；内部事件的 workspace_id 与 wire workspace 由 DTO 显式转换。create 不接收 revision，存储层以目标不存在为前置条件，不能覆盖已有或保留数据的 home。字段与当前 Protocol/DTO 接线一致；验证结论限于实际执行过的 scoped 与集成场景。

侧栏新建工作区必须使用 backend 目录选择能力取得 daemon 文件系统中的项目目录，再由创建操作重新验证其已存在、绝对且规范；不能以浏览器本地文件选择结果充当 backend 路径。新会话从 `state.sync.drivers` 选择所有已安装且完整验证的规范 Driver ID，包括尚未挂载的包；文件存在或当前已挂载实例列表不足以证明有效。扫描、目录选择、协议投影及持久新增已接通，并有 scoped 与隔离数据浏览器验证；未执行真实模型推理。

当前端点：`workspace.paths.query {path?: String, cursor?: String}` 返回 `workspace.paths {id, path: String, parent: String|null, directories: [{name, path}], next_cursor?: String}`。省略 path 使用 daemon HOME；响应路径均为后端规范目录，只列目录、不列文件、不 mkdir；必须有上限和分页，错误经匹配请求 id 的 `request.failed` 返回。共享 `QueryProjectDirectories`/Result 经注入的 `ProjectDirectoryBrowser` 阻塞能力在异步服务的 blocking worker 执行，DTO 只转换和关联，不做文件 IO。

当前 `driver.catalog.query`/`driver.catalog.refresh` 返回 `driver.catalog {id, drivers: [规范 Driver ID], errors: [{path, message}]}`；启动扫描发布共享 `DriverCatalog`，保留 `state.sync.drivers`。驱动领域校验完整包并报告逐包诊断，不把文件存在视为有效。这些端点及共享浏览能力已实现；驱动有效性限于路径、普通文件、有界 UTF-8、清单 schema/model/dependencies 校验，不编译或执行 Lua。

迁移实施顺序：持有完整生命周期 lease，并按确定顺序同时锁源/目标名称；核对磁盘 revision、目标不存在及全部路径在受控 home 内。工作区改名必须已停止；成员活实例先关闭受理、收口已受理创建并真实 drain/stop，随后关闭同一连接管理器的所有 blocks/history 句柄并处理 WAL/SHM。

Compose 文件存储内部按准备、提交、恢复协调迁移（没有独立注入的公开迁移服务）。内部 `workspaces/.operations/<operation-id>/` 保存 journal、staging 和提交时保留的 backup，不是 registry/index；journal 记录操作类型、源/目标名称、来源 revision、旧→新身份映射及源/暂存校验和，并同步文件和目录。Memory 从关闭句柄的原库通过 SQLite backup 生成一致副本，只在副本事务内更新 `block_definitions.agent`；定义 id、TEMP instance、schema、fields 和 history 的全部列/顺序原样校验保留。当前查询身份由重建的新 Agent/history 句柄投影，不改 history/audit 原始事实。工作区改名覆盖整个 home 的已知成员库，包括已移除但保留的库；成员改名不覆盖保留的同名目标库。未知 schema 或外来 namespace 明确拒绝，提交前保留可恢复原配置/数据。

commit 只发布校验通过的副本，持久写入 `committed` 决策后按 journal 安装目录/成员库与 YAML，保留 backup 并同步目录，以校验和确认安装后写入 `complete`。多 DB、文件、目录与 ECS 不具整体原子性；中断由 recovery 按已确认阶段协调，失败可返回含 `commit_unknown` / `recovery_required` 的诊断，不开空库或自动重启假装回滚。对新身份持久上下文和历史可访问性的验证另由真实 FileStore+Driver 集成测试覆盖。

成员改名的运行策略：若父 Workspace 为 Running 且目标成员有活实例，先真实 drain/stop 并关闭全部数据库视图，再 prepare/commit typed 身份迁移；只有持久改名确认成功且父 Workspace 仍为 Running，才按新名称/新 ID 尝试启动该成员。完整操作 lease 覆盖这次启动结果收口；持久成功后启动失败仍返回新 catalog 条目及已提交 revision，保留改名结果，通过新 ID 的 member_failures、日志和侧栏红点报告运行失败，不回滚名称。原本已停止或无活实例的成员只改持久身份，不自动启动；工作区自身改名始终仅允许停止状态。持久失败不启动新身份，也不能声称旧成员仍在运行；停止成功后失败须报告已停止、当前确认配置/身份及必要 recovery_required 状态。不得在 durable commit 前重启。

文件存储构造及后续受理检查执行内部 recovery；`.operations`、`.trash` 与内部暂存项不参与 catalog 枚举。journal 使用 `preparing`、`prepared`、`committed`、`complete`、`rolled_back` 阶段：未提交且源校验通过时回滚准备阶段，已提交时幂等完成安装。未知、损坏或无法安全恢复的 journal 会设置存储级 `recovery_required`，保守阻止所有 catalog-backed 加载、启动和变更，而非仅封锁源/目标；诊断目录仍可列出。不能静默打开部分新身份或按任意 journal 路径写入。已有阶段中断及重复恢复测试不等于穷举磁盘故障。

## 持久定义与 catalog

- 默认规范位置为 `~/.margatroid/workspaces/<name>/workspace.yaml`；项目目录不创建 `.margatroid`。成员数据库固定为同目录下 `agents/<member>/state.sqlite`，blocks 与 history 继续共用现有数据库管理器。
- `WorkspaceDefinition` 没有 manager 字段或必需的特殊成员；空成员列表合法。
- `RefreshWorkspaceCatalog` 调用共享 `WorkspaceCatalogStore::list`，发布包含停止和无效定义条目的快照；`QueryWorkspaceCatalog` 只查询最近快照，不执行磁盘操作。
- 所有 list/load/save/rename_workspace/delete_workspace/rename_member 调用均在异步服务中的 blocking worker 执行，不持有 ECS World 借用，也不在主线程处理文件 I/O 或 YAML。
- `StartRegisteredWorkspace { id, workspace_id, expected_revision }` 由 daemon 从规范存储加载编译定义。客户端不提供任意数据库路径。`StartWorkspace` 保留为可信内部已编译定义入口及运行期测试入口。
- 运行状态、Entity、线程、等待表和成员失败仅存在于运行期投影，不写回 YAML。refresh 发现运行定义与磁盘定义不同时标注冲突；成员变更拒绝采用该变化，必须先显式停止，不隐式关闭整个工作区。

## 成员变更

`AddWorkspaceMember` 接收 workspace ID、非空 expected revision，以及 `WorkspaceMemberSpec { name, driver, home_root }`。生命周期从规范条目按名称推导成员 ID 和数据库位置（名称未变时稳定，改名须遵循上述迁移要求）；省略 home_root 时采用 daemon 的 HOME。先确认 save 成功，再为运行工作区加载驱动并启动新成员；停止的工作区只持久化，不启动线程或成员。运行启动失败不撤销已提交配置，通过 `WorkspaceMemberStartFailed` 和运行状态投影报告；添加结果的 Ok 表示配置提交成功。

`RemoveWorkspaceMember` 先确认没有未完成创建，再对活成员发送普通 `StopAgent`，等待真实 drain/stop 成功后才保存移除。停止失败保留配置和运行引用。停止成功但 save 失败时保留原定义并返回错误，明确报告成员实际已经停止；不自动重启伪装回滚。数据库始终保留，同名重新添加恢复原持久数据。移除最后一个成员合法，工作区执行线程继续运行。

每工作区完整操作持有 busy lease，覆盖加载、保存、驱动加载、成员启动、成员停止及 drain 等待。start、stop、成员变更及 refresh 不得重叠；重复请求不能覆盖或释放已受理操作。与已受理动态创建重叠的成员编辑返回 WorkspaceBusy，调用方待创建收口后显式重试。revision 由存储 backend 逐次核对，陈旧请求不能静默覆盖新定义。catalog-backed 世界的动态启动入口还要求成员及其驱动、数据库路径与已持久化定义完全一致。

YAML、SQLite 与 ECS 没有分布式原子性保证。文件失败、停止失败、启动失败分别报告；不能把部分完成包装成整体回滚成功。

## 运行实例

初始启动按驱动归并 `LoadAgentDriver`，加载成功后发送 `StartAgent`。全部初始请求结束后进入 Running；一个或全部成员启动失败均不阻断工作区。成功成员保存在 agents，失败保存在 member_failures，每个失败产生带完整工作区、成员、驱动和原始错误的 `WorkspaceMemberStartFailed`。

`StopWorkspace` 只受理 Running，关闭提交门，等待已受理动态创建和所有成员停止，再异步等待执行线程退出，最后回收运行 Entity。真实清理或线程退出失败保留 Failed 实例，不吞错误。停止不会删除 catalog。

本次 19 项单元测试覆盖运行期生命周期、blocking storage 不在主线程、重复变更/refresh busy、移除停止成功后保存失败、运行磁盘定义冲突及上述 CRUD 协调。结果仅覆盖本 crate 生命周期与替身存储，不替代持久迁移、驱动扫描、目录选择或 UI 链路验收。
