# MargatroidDaemon

## 当前实现与目录契约

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

组合根注入共享 catalog storage。可变定义为 `~/.margatroid/workspaces/<name>/workspace.yaml`，成员 blocks/history 共用 `agents/<agent>/state.sqlite`。`project_root` 必须显式指定已存在的绝对规范目录，项目内不创建 `.margatroid`。无 manager、隐式成员路由或数据库路径覆盖；空成员定义有效。启动只刷新 catalog，不自动启动工作区。内部 `.trash` 与暂存项不参与 catalog。

添加先提交定义再尝试启动，启动失败保留配置；移除先真实停止再持久化，停止后保存失败报告已停止/仍配置，不自动重启假装回滚。操作持有完整 lease 与磁盘 revision 检查。工作区改名/删除要求停止且无实例/未收口任务；工作区删除移动整个 home 到 `workspaces/.trash/<unique>`，成员删除保留 DB。继续保留按名称派生的 ResourceID。

当前网络 payload（外层请求 id 仅作关联）：`workspace.create {name, project_root}`；`workspace.rename {workspace, expected_revision, new_name}`；`workspace.delete {workspace, expected_revision}`；`workspace.member.rename {workspace, agent, expected_revision, new_name}`。workspace/agent 均为规范 ResourceID，成员改名同时核对归属；wire 统一使用 snake_case expected_revision；内部事件的 workspace_id 与 wire workspace 由 DTO 显式转换。create 不接收 revision，存储层以目标不存在为前置条件，不能覆盖已有或保留数据的 home。字段与当前 Protocol/DTO 接线一致；验证结论限于实际执行过的 scoped 与集成场景。

侧栏新建工作区必须使用 backend 目录选择能力取得 daemon 文件系统中的项目目录，再由创建操作重新验证其已存在、绝对且规范；不能以浏览器本地文件选择结果充当 backend 路径。新会话从 `state.sync.drivers` 选择所有已安装且完整验证的规范 Driver ID，包括尚未挂载的包；文件存在或当前已挂载实例列表不足以证明有效。扫描、目录选择、协议投影及持久新增已接通，并有 scoped 与隔离数据浏览器验证；未执行真实模型推理。

当前端点：`workspace.paths.query {path?: String, cursor?: String}` 返回 `workspace.paths {id, path: String, parent: String|null, directories: [{name, path}], next_cursor?: String}`。省略 path 使用 daemon HOME；响应路径均为后端规范目录，只列目录、不列文件、不 mkdir；必须有上限和分页，错误经匹配请求 id 的 `request.failed` 返回。共享 `QueryProjectDirectories`/Result 经注入的 `ProjectDirectoryBrowser` 阻塞能力在异步服务的 blocking worker 执行，DTO 只转换和关联，不做文件 IO。

当前 `driver.catalog.query`/`driver.catalog.refresh` 返回 `driver.catalog {id, drivers: [规范 Driver ID], errors: [{path, message}]}`；启动扫描发布共享 `DriverCatalog`，保留 `state.sync.drivers`。驱动领域校验完整包并报告逐包诊断，不把文件存在视为有效。这些端点及共享浏览能力已实现；驱动有效性限于路径、普通文件、有界 UTF-8、清单 schema/model/dependencies 校验，不编译或执行 Lua。

迁移实施顺序：持有完整生命周期 lease，并按确定顺序同时锁源/目标名称；核对磁盘 revision、目标不存在及全部路径在受控 home 内。工作区改名必须已停止；成员活实例先关闭受理、收口已受理创建并真实 drain/stop，随后关闭同一连接管理器的所有 blocks/history 句柄并处理 WAL/SHM。

Compose 文件存储内部按准备、提交、恢复协调迁移（没有独立注入的公开迁移服务）。内部 `workspaces/.operations/<operation-id>/` 保存 journal、staging 和提交时保留的 backup，不是 registry/index；journal 记录操作类型、源/目标名称、来源 revision、旧→新身份映射及源/暂存校验和，并同步文件和目录。Memory 从关闭句柄的原库通过 SQLite backup 生成一致副本，只在副本事务内更新 `block_definitions.agent`；定义 id、TEMP instance、schema、fields 和 history 的全部列/顺序原样校验保留。当前查询身份由重建的新 Agent/history 句柄投影，不改 history/audit 原始事实。工作区改名覆盖整个 home 的已知成员库，包括已移除但保留的库；成员改名不覆盖保留的同名目标库。未知 schema 或外来 namespace 明确拒绝，提交前保留可恢复原配置/数据。

commit 只发布校验通过的副本，持久写入 `committed` 决策后按 journal 安装目录/成员库与 YAML，保留 backup 并同步目录，以校验和确认安装后写入 `complete`。多 DB、文件、目录与 ECS 不具整体原子性；中断由 recovery 按已确认阶段协调，失败可返回含 `commit_unknown` / `recovery_required` 的诊断，不开空库或自动重启假装回滚。对新身份持久上下文和历史可访问性的验证另由真实 FileStore+Driver 集成测试覆盖。

成员改名的运行策略：若父 Workspace 为 Running 且目标成员有活实例，先真实 drain/stop 并关闭全部数据库视图，再 prepare/commit typed 身份迁移；只有持久改名确认成功且父 Workspace 仍为 Running，才按新名称/新 ID 尝试启动该成员。完整操作 lease 覆盖这次启动结果收口；持久成功后启动失败仍返回新 catalog 条目及已提交 revision，保留改名结果，通过新 ID 的 member_failures、日志和侧栏红点报告运行失败，不回滚名称。原本已停止或无活实例的成员只改持久身份，不自动启动；工作区自身改名始终仅允许停止状态。持久失败不启动新身份，也不能声称旧成员仍在运行；停止成功后失败须报告已停止、当前确认配置/身份及必要 recovery_required 状态。不得在 durable commit 前重启。

文件存储构造及后续受理检查执行内部 recovery；`.operations`、`.trash` 与内部暂存项不参与 catalog 枚举。journal 使用 `preparing`、`prepared`、`committed`、`complete`、`rolled_back` 阶段：未提交且源校验通过时回滚准备阶段，已提交时幂等完成安装。未知、损坏或无法安全恢复的 journal 会设置存储级 `recovery_required`，保守阻止所有 catalog-backed 加载、启动和变更，而非仅封锁源/目标；诊断目录仍可列出。不能静默打开部分新身份或按任意 journal 路径写入。已有阶段中断及重复恢复测试不等于穷举磁盘故障。

## 函数

私有：
```text
main()
    daemon入口：调用run并将错误转换为进程退出码；不解析任何启动参数

run() -> Result<(), Error>
    组合并启动应用：私有函数，打开主目录中的插件资源，按依赖顺序安装Plugin并运行App
    行为：
        从HOME固定得到~/.margatroid并创建主目录
        检查models.toml和config.toml存在
        加载并验证config.toml中的server.bind、server.allow、日志配置和全部出站配置
        使用server.bind构造ServerPlugin，并把server.allow解析出的入口策略作为握手判定接缝注入
        使用log.level和可选的log.filter构造LogPlugin；filter存在时优先于level
        将全局只读WebSocket目标配置交给DtoPlugin和InferencePlugin
        打开AgentDriver、Workspace、ToolPlugin和LuaRuntimePlugin所需目录
        注入WorkspaceCatalogHandle与ProjectDirectoryBrowserHandle；迁移恢复由具体catalog store内部实现
        FileWorkspaceCatalogStore构造时恢复journal；恢复失败设置全store recovery_required，阻止catalog-backed加载/变更
        由loader异步启动扫描已安装Driver，发布DriverCatalog及逐包诊断（不编译/执行Lua）
        安装运行时、日志、Server和全部领域Plugin
        安装DtoPlugin和ClientPlugin
        记录启动信息并调用AppRunExt::run

data_root() -> Result<PathBuf, Error>
    构造固定主目录：私有函数，返回HOME下的.margatroid；HOME缺失时启动失败

log_level(level: ConfigLogLevel) -> LogLevel
    转换日志级别：私有函数，把config_plugin的LogLevel逐项映射为log_plugin的LogLevel
    行为：只做同词表转换，不校验取值；取值合法性已由ConfigPlugin在加载期保证
```

## 逻辑

```text
main
    -> run
        -> 打开~/.margatroid/config.toml
        -> 使用server.bind与入口策略安装ServerPlugin
        -> 使用log.level与log.filter安装LogPlugin
        -> 按依赖顺序安装ToolPlugin、LuaRuntimePlugin、MclPlugin等全部Plugin
        -> AppRunExt::run

边界：
    daemon负责固定主目录定位、Plugin配置与装配、进程退出
    daemon不接受启动参数；运行配置以~/.margatroid/config.toml为准
    daemon不定义或注册业务System
    daemon不解析API消息，不路由Workspace或Agent，不构造前端状态，不转发日志
    API DTO与领域命令转换、领域状态和日志的客户端投影均由DtoPlugin负责
    tracing Subscriber每进程只安装一次，因此日志级别必须在安装LogPlugin之前确定
```

## 运行限制

入口 allow 与连接注册不是独立服务端认证/授权系统；开放监听须自行部署可信访问边界。当前没有 runtime graceful shutdown API，不应将进程退出描述为完整运行清理保证。不承诺硬性响应延迟，本地 HTTP 取消也不保证供应商停止计算或计费。
