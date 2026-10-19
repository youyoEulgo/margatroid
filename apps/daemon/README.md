# Margatroid Daemon

Daemon 是产品组合根，读取主目录配置、构造共享存储能力、安装插件并运行 App。默认使用 `~/.margatroid`，不接受启动参数。

```text
cargo run -p margatroid_daemon
```


## 当前实现与边界

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

侧栏实现为 Workspace 可展开树，Agent 是子行；两级行左侧状态点（灰未启动、绿正常、红出错），右侧 `…` 仅有 Rename/Delete。每个工作区包含与 Agent 并列的 `+ New session`（名称＋选择已安装 AgentDriver），顶层包含 `+ New workspace`（名称＋daemon 项目路径）。加号行无 `…`。只移除左下角重复 MCL 快捷入口，右侧原有审计查看能力不因此删除。

当前请求为 workspace.create、workspace.rename、workspace.delete、workspace.member.rename，具体字段以 Protocol 文档为准；当前已接通并有 scoped 和隔离数据浏览器验证。创建工作区只登记空定义。Workspace 重命名/删除必须先停止；成员重命名必须等待活成员 drain/stop。删除 Workspace 默认移动整个 home 到内部 `.trash` 回收区，不出现在 catalog，删除 Agent 默认保留数据库。重命名会改变现有 ResourceID，因此必须同时解决 Block namespace 与历史身份的持久迁移；只移动文件或只更新 YAML 不算完成，更不能宣称保留会话。

新会话选择的 image 指 AgentDriver 包，通过 `state.sync.drivers` 暴露可用、有效、规范化的 Driver ID，包含尚未挂载的包，不暴露脚本、模型密钥或执行实例。扫描、协议投影与持久新增已接通；验证不包含真实模型推理。

当前网络 payload（外层请求 id 仅作关联）：`workspace.create {name, project_root}`；`workspace.rename {workspace, expected_revision, new_name}`；`workspace.delete {workspace, expected_revision}`；`workspace.member.rename {workspace, agent, expected_revision, new_name}`。workspace/agent 均为规范 ResourceID，成员改名同时核对归属；wire 统一使用 snake_case expected_revision；内部事件的 workspace_id 与 wire workspace 由 DTO 显式转换。create 不接收 revision，存储层以目标不存在为前置条件，不能覆盖已有或保留数据的 home。字段与当前 Protocol/DTO 接线一致；验证结论限于实际执行过的 scoped 与集成场景。

侧栏新建工作区必须使用 backend 目录选择能力取得 daemon 文件系统中的项目目录，再由创建操作重新验证其已存在、绝对且规范；不能以浏览器本地文件选择结果充当 backend 路径。新会话从 `state.sync.drivers` 选择所有已安装且完整验证的规范 Driver ID，包括尚未挂载的包；文件存在或当前已挂载实例列表不足以证明有效。扫描、目录选择、协议投影及持久新增已接通，并有 scoped 与隔离数据浏览器验证；未执行真实模型推理。

当前端点：`workspace.paths.query {path?: String, cursor?: String}` 返回 `workspace.paths {id, path: String, parent: String|null, directories: [{name, path}], next_cursor?: String}`。省略 path 使用 daemon HOME；响应路径均为后端规范目录，只列目录、不列文件、不 mkdir；必须有上限和分页，错误经匹配请求 id 的 `request.failed` 返回。共享 `QueryProjectDirectories`/Result 经注入的 `ProjectDirectoryBrowser` 阻塞能力在异步服务的 blocking worker 执行，DTO 只转换和关联，不做文件 IO。

当前 `driver.catalog.query`/`driver.catalog.refresh` 返回 `driver.catalog {id, drivers: [规范 Driver ID], errors: [{path, message}]}`；启动扫描发布共享 `DriverCatalog`，保留 `state.sync.drivers`。驱动领域校验完整包并报告逐包诊断，不把文件存在视为有效。这些端点及共享浏览能力已实现；驱动有效性限于路径、普通文件、有界 UTF-8、清单 schema/model/dependencies 校验，不编译或执行 Lua。

迁移实施顺序：持有完整生命周期 lease，并按确定顺序同时锁源/目标名称；核对磁盘 revision、目标不存在及全部路径在受控 home 内。工作区改名必须已停止；成员活实例先关闭受理、收口已受理创建并真实 drain/stop，随后关闭同一连接管理器的所有 blocks/history 句柄并处理 WAL/SHM。

Compose 文件存储内部按准备、提交、恢复协调迁移（没有独立注入的公开迁移服务）。内部 `workspaces/.operations/<operation-id>/` 保存 journal、staging 和提交时保留的 backup，不是 registry/index；journal 记录操作类型、源/目标名称、来源 revision、旧→新身份映射及源/暂存校验和，并同步文件和目录。Memory 从关闭句柄的原库通过 SQLite backup 生成一致副本，只在副本事务内更新 `block_definitions.agent`；定义 id、TEMP instance、schema、fields 和 history 的全部列/顺序原样校验保留。当前查询身份由重建的新 Agent/history 句柄投影，不改 history/audit 原始事实。工作区改名覆盖整个 home 的已知成员库，包括已移除但保留的库；成员改名不覆盖保留的同名目标库。未知 schema 或外来 namespace 明确拒绝，提交前保留可恢复原配置/数据。

commit 只发布校验通过的副本，持久写入 `committed` 决策后按 journal 安装目录/成员库与 YAML，保留 backup 并同步目录，以校验和确认安装后写入 `complete`。多 DB、文件、目录与 ECS 不具整体原子性；中断由 recovery 按已确认阶段协调，失败可返回含 `commit_unknown` / `recovery_required` 的诊断，不开空库或自动重启假装回滚。对新身份持久上下文和历史可访问性的验证另由真实 FileStore+Driver 集成测试覆盖。

成员改名的运行策略：若父 Workspace 为 Running 且目标成员有活实例，先真实 drain/stop 并关闭全部数据库视图，再 prepare/commit typed 身份迁移；只有持久改名确认成功且父 Workspace 仍为 Running，才按新名称/新 ID 尝试启动该成员。完整操作 lease 覆盖这次启动结果收口；持久成功后启动失败仍返回新 catalog 条目及已提交 revision，保留改名结果，通过新 ID 的 member_failures、日志和侧栏红点报告运行失败，不回滚名称。原本已停止或无活实例的成员只改持久身份，不自动启动；工作区自身改名始终仅允许停止状态。持久失败不启动新身份，也不能声称旧成员仍在运行；停止成功后失败须报告已停止、当前确认配置/身份及必要 recovery_required 状态。不得在 durable commit 前重启。

文件存储构造及后续受理检查执行内部 recovery；`.operations`、`.trash` 与内部暂存项不参与 catalog 枚举。journal 使用 `preparing`、`prepared`、`committed`、`complete`、`rolled_back` 阶段：未提交且源校验通过时回滚准备阶段，已提交时幂等完成安装。未知、损坏或无法安全恢复的 journal 会设置存储级 `recovery_required`，保守阻止所有 catalog-backed 加载、启动和变更，而非仅封锁源/目标；诊断目录仍可列出。不能静默打开部分新身份或按任意 journal 路径写入。已有阶段中断及重复恢复测试不等于穷举磁盘故障。

## 主目录与工作区

```text
~/.margatroid/
├── config.toml
├── models.toml
├── agent-drivers/
├── tools/
├── prompts/
├── sandboxes/
└── workspaces/
    └── <workspace-name>/
        ├── workspace.yaml
        └── agents/
            └── <agent-name>/
                └── state.sqlite
```

`workspace.yaml` 是可变的成员定义权威，目录名与 name 一致，名称全局唯一；公开 ResourceID 由名称派生，改名必须迁移身份及数据关联。项目只提供工具工作路径；正常流程不创建项目级 `.margatroid` 或配置文件。`state.sqlite` 同时保存既有 Block、会话和历史，不拆库，YAML 不接受 history_path/memory_path 覆盖。home_root 仍指成员使用的用户主目录。

```yaml
name: margatroid-test
project_root: /home/eulgo/Documents/multi/margatroid/demo_workspace
agents:
  coder:
    driver: local/coder:latest
  reviewer:
    driver: local/reviewer:latest
```

project_root 必须是已存在、绝对且规范化的项目路径；不自动创建或回退到配置目录。manager 已移除，空成员工作区合法，成员请求必须显式指定 Agent ID。多个工作区可对应同一项目路径，状态互相独立；修改项目路径不迁移数据库。

组合根构造 Compose 的 `FileWorkspaceCatalogStore`，以 `WorkspaceCatalogHandle` 注入共享能力。daemon 启动发送目录刷新事件，固定读取 `workspaces/*/workspace.yaml`，不自动启动任何工作区；目录服务在 blocking worker 中读写 YAML，ECS 系统不做文件 IO。目录缺失可创建主目录下的 workspaces；坏定义或缺失配置的工作区 home 仍显示诊断，其他条目正常工作。不另设索引文件；内部 `.catalog.lock` 仅为存储并发锁。

CLI 注册后查询 daemon 权威目录，`workspace up [NAME]` 以名称或规范化当前目录与 project_root 的精确匹配选择：0 项报错，1 项直接启动，多项交互选择，非交互多项要求名称。取消选择不启动，不搜索祖先目录、不自动登记。启动请求只提交工作区身份和预期修订号，后端读取并编译规范定义；不能上传任意 YAML 或数据库路径。同目录匹配要求客户端和后端共享文件系统命名空间，远程使用显式名称。CLI 保持连接、显示日志、Ctrl+C 请求停止并等待清理回执。

## 成员持久化与失败

加入成员先校验修订并持久登记，然后仅在工作区已运行时尝试启动；停止状态编辑只修改配置。启动失败保留配置和 member_failures，前端显示配置与运行事实。移除先完成真实成员停止/drain，再写回定义；默认保留数据库，同名再次加入可恢复既有数据。普通停止工作区不取消注册，不删除数据库。

每个工作区完整操作持有生命周期租约，文件存储以进程内锁及跨进程文件锁协调，以内容 SHA-256 为修订。同目录临时文件、文件同步、原子替换和目录同步发布定义；写入前重新核对来源防止覆盖过期修改。YAML、SQLite 与 ECS 没有跨介质事务：停止成功但持久删除失败时报告已停止/仍配置，不伪造成功或回滚。运行中刷新发现磁盘成员定义与实例不符时报告冲突，要求显式停止后重载，不偷偷替换运行集合。SQLite 是否存在不代表工作区正在运行。

目录有效性与运行状态独立，前端收到 `state.sync.catalog` 可发现 stopped、invalid 及 live 工作区；runtime workspaces/agents 仍只表示真实实例。启动成员失败不会使 Workspace 整体失败，即使全部成员失败仍可正常停止；真正创建/执行线程失败不留下新占位，重复启动保留原实例并给出状态诊断。

迁移必须先停止相关工作区并关闭数据库连接，正确处理 WAL/SHM，再迁移完整 Agent 数据库并校验重开。成功前保留原配置/数据；启动不自动扫描、搬迁或删除项目遗留文件。

## 全局配置与协议

`models.toml` 必须存在，InferencePlugin 校验 provider + api_type 路由。`config.toml` 配置监听地址、入口策略以及日志与状态两类出站目标：

```toml
[server]
bind = "127.0.0.1:3939"
allow = "localhost"

[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]
```

默认仅接受回环对端，携带 Origin 时校验 Host 同源；放宽 allow 需与 bind 一致。目标支持 broadcast、type 与 name，完整规则见 ConfigPlugin。客户端先 connection.register，再查询/刷新目录及启动，或向明确 Agent 投递 MCL/中止/历史请求。workspace.member.add/remove 使用预期修订，响应区分持久失败与成员运行失败。

`state.sync` 是权威目录、运行状态和历史快照，只在内容或目标变化时发布；历史来自每个 Agent 同一 SQLite 的 history 视图。前端不乐观拼接最终对话、不以流结果替代持久历史。

## 资源与推理

仓库 resources 包含 coder、reviewer Driver 及工具/提示词/沙箱；使用时部署到主目录对应路径。Driver 清单使用顶层 dependencies 字符串数组。每个 Agent 拥有独立 Lua、会话及数据库。reviewer 默认只读；coder 的 read_image 使用当前沙箱读取图片，旧持久 session.visible 不被启动脚本覆盖，需显式导入启用新工具。

共享纯 DomainRequest 经三种 API 格式和厂商适配器构造 HTTP。支持七种明确组合：OpenAI chat/responses、Anthropic messages、DeepSeek 三种、OpenRouter chat。没有 URL 猜测或隐式厂商回退。Responses 每次本地完整上下文且 store=false；replay 保留 Messages 签名/遮蔽思考和 Responses 加密 reasoning，跨格式或不可表达参数明确拒绝。

User 与 ToolCall images 独立于文本；Messages/Responses 原生工具图片，Chat 等整批工具回执齐全再附加 User 图片，只改 HTTP 格式，不改本地工具历史。图片格式支持不保证上游模型具备视觉能力。ToolPlugin 通过独立 runner 执行 Lua 工具，shell 是工具 ABI 而非资源。

InferencePlugin 显式安装在 UPDATE，用户中止 PRE_UPDATE 先于本帧 admission。取消已运行推理时 abort 并 join 本地 HTTP/SSE worker，累计部分输出保存为 cancelled，只入历史不入模型上下文；无输出时保持空 Assistant，不填文案。前端点击即冻结该轮显示，不染红、不添加取消标签，但不会提前伪装 Idle。工具真实清理失败仍报告失败。本地断连不保证供应商停止远端计算或计费。

## 运行限制

入口 allow 与连接注册不是独立服务端认证/授权系统；开放监听须自行部署可信访问边界。当前没有 runtime graceful shutdown API，不应将进程退出描述为完整运行清理保证。不承诺硬性响应延迟，本地 HTTP 取消也不保证供应商停止计算或计费。
