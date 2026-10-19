# Margatroid Types

`margatroid_types` 保存多个 Margatroid Plugin 共享、没有业务执行逻辑的领域值和内部事件格式。
它可以依赖 `core_plugin` 的 `Entity` 与 `Event`，但不依赖任何 Margatroid 业务 Plugin，也不承载
CLI/daemon 网络 DTO。

资源层提供统一的 `ResourceID`。所有可寻址对象都视为资源，不论它是否由 ECS Entity
承载：

```rust
use margatroid_types::ResourceID;

let tool = ResourceID::parse("tool:local/code-review")?;
assert_eq!(tool.to_string(), "tool:local/code-review:latest");

let driver = ResourceID::parse("driver:local/coder:latest")?;
assert_eq!(driver.resource_type(), "driver");
```

资源ID固定使用 `type:scope/name:tag`。省略tag时规范化为 `latest`。Agent、Workspace、AgentDriver、
Prompt、Sandbox和Tool都使用同一种身份格式。`ResourceID`是跨Plugin查找资源的统一地址；
`core_plugin::Entity`只作为当前World内的运行时句柄，不进入协议、日志或Memory。

Compose 使用 `WorkspaceDefinition` 和 `WorkspaceAgentDefinition` 把规范配置编译成进程内业务输入，不包含运行时 Entity，也不加载驱动、打开数据库或创建 AgentInstance。
当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。


## 持久工作区共享契约（已实现）

新建工作区、工作区改名/删除、成员改名及驱动扫描均已实现。Sidebar 输入名称和已存在的绝对规范项目目录来新建空工作区，输入会话名称并显式选择已有 AgentDriver 来新增普通成员；工作区/会话提供改名与删除菜单。驱动使用 `driver:scope/name:tag`，不是 `agent-driver:`；启动扫描有效已安装包，包含尚未挂载的包，没有默认驱动或自动加载语义。

改名必须保留全部持久 blocks/history 的内容、可访问性与关联。名称目前参与 ResourceID 与数据库命名空间，改文件路径或 ID 不足以保持数据连续。工作区改名要求已停止且无存活实例；运行成员先关闭新受理、等待创建收口并真实 drain/stop，关闭所有数据库句柄与同连接管理器的 blocks/history 视图后才能迁移身份/路径。目标确定保留按名称派生的公开 ResourceID，本范围不引入新的稳定身份模型。重命名必须在全部句柄关闭后，显式迁移旧 ResourceID 的已知 typed 持久命名空间及当前 history 查询关联到新 ResourceID；原始事实性 history/audit 内容保持原样，不盲目改写。当前迁移实现与对应 scoped 测试覆盖该契约。

工作区删除要求停止且无存活实例及未收口任务，完整 home 移至内部 `workspaces/.trash/<unique>` 并排除 catalog，不做不可逆数据库删除；成员删除仍保留数据库。所有写操作持有完整 lease 与 expected revision，新建由存储层检查目标不存在（不是 create 请求字段）；改名保护源/目标名称。冲突在写入前拒绝，旧数据保持原样。多库、目录、YAML 与 ECS 无整体原子性；身份迁移需可恢复阶段记录及明确的部分完成/提交未知结果。

当前网络 payload（外层请求 id 仅作关联）：`workspace.create {name, project_root}`；`workspace.rename {workspace, expected_revision, new_name}`；`workspace.delete {workspace, expected_revision}`；`workspace.member.rename {workspace, agent, expected_revision, new_name}`。workspace/agent 均为规范 ResourceID，成员改名同时核对归属；wire 统一使用 snake_case expected_revision；内部事件的 workspace_id 与 wire workspace 由 DTO 显式转换。create 不接收 revision，存储层以目标不存在为前置条件，不能覆盖已有或保留数据的 home。字段与当前 Protocol/DTO 接线一致；验证结论限于实际执行过的 scoped 与集成场景。

侧栏新建工作区必须使用 backend 目录选择能力取得 daemon 文件系统中的项目目录，再由创建操作重新验证其已存在、绝对且规范；不能以浏览器本地文件选择结果充当 backend 路径。新会话从 `state.sync.drivers` 选择所有已安装且完整验证的规范 Driver ID，包括尚未挂载的包；文件存在或当前已挂载实例列表不足以证明有效。扫描、目录选择、协议投影及持久新增已接通，并有 scoped 与隔离数据浏览器验证；未执行真实模型推理。

当前端点：`workspace.paths.query {path?: String, cursor?: String}` 返回 `workspace.paths {id, path: String, parent: String|null, directories: [{name, path}], next_cursor?: String}`。省略 path 使用 daemon HOME；响应路径均为后端规范目录，只列目录、不列文件、不 mkdir；必须有上限和分页，错误经匹配请求 id 的 `request.failed` 返回。共享 `QueryProjectDirectories`/Result 经注入的 `ProjectDirectoryBrowser` 阻塞能力在异步服务的 blocking worker 执行，DTO 只转换和关联，不做文件 IO。

当前 `driver.catalog.query`/`driver.catalog.refresh` 返回 `driver.catalog {id, drivers: [规范 Driver ID], errors: [{path, message}]}`；启动扫描发布共享 `DriverCatalog`，保留 `state.sync.drivers`。驱动领域校验完整包并报告逐包诊断，不把文件存在视为有效。这些端点及共享浏览能力已实现；驱动有效性限于路径、普通文件、有界 UTF-8、清单 schema/model/dependencies 校验，不编译或执行 Lua。

迁移实施顺序：持有完整生命周期 lease，并按确定顺序同时锁源/目标名称；核对磁盘 revision、目标不存在及全部路径在受控 home 内。工作区改名必须已停止；成员活实例先关闭受理、收口已受理创建并真实 drain/stop，随后关闭同一连接管理器的所有 blocks/history 句柄并处理 WAL/SHM。

Compose 文件存储内部按准备、提交、恢复协调迁移（没有独立注入的公开迁移服务）。内部 `workspaces/.operations/<operation-id>/` 保存 journal、staging 和提交时保留的 backup，不是 registry/index；journal 记录操作类型、源/目标名称、来源 revision、旧→新身份映射及源/暂存校验和，并同步文件和目录。Memory 从关闭句柄的原库通过 SQLite backup 生成一致副本，只在副本事务内更新 `block_definitions.agent`；定义 id、TEMP instance、schema、fields 和 history 的全部列/顺序原样校验保留。当前查询身份由重建的新 Agent/history 句柄投影，不改 history/audit 原始事实。工作区改名覆盖整个 home 的已知成员库，包括已移除但保留的库；成员改名不覆盖保留的同名目标库。未知 schema 或外来 namespace 明确拒绝，提交前保留可恢复原配置/数据。

commit 只发布校验通过的副本，持久写入 `committed` 决策后按 journal 安装目录/成员库与 YAML，保留 backup 并同步目录，以校验和确认安装后写入 `complete`。多 DB、文件、目录与 ECS 不具整体原子性；中断由 recovery 按已确认阶段协调，失败可返回含 `commit_unknown` / `recovery_required` 的诊断，不开空库或自动重启假装回滚。对新身份持久上下文和历史可访问性的验证另由真实 FileStore+Driver 集成测试覆盖。

成员改名的运行策略：若父 Workspace 为 Running 且目标成员有活实例，先真实 drain/stop 并关闭全部数据库视图，再 prepare/commit typed 身份迁移；只有持久改名确认成功且父 Workspace 仍为 Running，才按新名称/新 ID 尝试启动该成员。完整操作 lease 覆盖这次启动结果收口；持久成功后启动失败仍返回新 catalog 条目及已提交 revision，保留改名结果，通过新 ID 的 member_failures、日志和侧栏红点报告运行失败，不回滚名称。原本已停止或无活实例的成员只改持久身份，不自动启动；工作区自身改名始终仅允许停止状态。持久失败不启动新身份，也不能声称旧成员仍在运行；停止成功后失败须报告已停止、当前确认配置/身份及必要 recovery_required 状态。不得在 durable commit 前重启。

文件存储构造及后续受理检查执行内部 recovery；`.operations`、`.trash` 与内部暂存项不参与 catalog 枚举。journal 使用 `preparing`、`prepared`、`committed`、`complete`、`rolled_back` 阶段：未提交且源校验通过时回滚准备阶段，已提交时幂等完成安装。未知、损坏或无法安全恢复的 journal 会设置存储级 `recovery_required`，保守阻止所有 catalog-backed 加载、启动和变更，而非仅封锁源/目标；诊断目录仍可列出。不能静默打开部分新身份或按任意 journal 路径写入。已有阶段中断及重复恢复测试不等于穷举磁盘故障。

daemon 组合根提供数据根（默认 `~/.margatroid`）和共享 typed catalog storage 能力。规范定义为 `~/.margatroid/workspaces/<name>/workspace.yaml`，是**可变的成员定义**；成员数据为 `agents/<agent>/state.sqlite`，继续使用现有 blocks/history 单库及同连接管理器。项目中不建立 `.margatroid`，`project_root` 不决定数据库根；名称须为安全目录段。manager 概念、字段、特殊角色和隐式路由全部移除，空成员列表合法。

共享层已有 `WorkspaceCatalogEntry`、`WorkspaceCatalogStore`（list/load/save）、`WorkspaceCatalogHandle`、`WorkspaceCatalog` 及成员变更事件；`WorkspaceDeletion`、rename/delete 存储方法与 create/rename/delete 请求结果均已实现。运行实例独立；状态、Entity、线程、member_failures、turn、等待表等不进入 YAML。旧运行态 WorkspaceRegistry 不恢复，持久 catalog 不是它的替代生命周期镜像。

Workspace Plugin 协调每工作区完整变更串行化及 expected revision 检查：添加先提交定义再尝试启动（仅正在运行的工作区），停止工作区只改配置而不自动启动，启动失败仍保留配置；移除先 drain/stop 活成员再提交定义移除。停止失败保留配置；停止成功但写入失败保留配置，显式报告已停止/仍配置的降级事实。默认保留数据库，同名再加入恢复持久 blocks/history，TEMP 按新实例隔离、turn 不恢复，其他沿用已有持久化规则。结果分开表达提交和运行事实，提交不明显式报告未知并重读，不伪称成功。

存储 backend 固定枚举数据根 `workspaces` 一级目录，调用 Compose 所属的纯 parser/file codec、承担文件 I/O，以同目录临时文件原子 rename 提交；所有写者须遵循同一串行锁/revision 协议防止丢失更新。共享 types 不处理 YAML，System 不硬编码路径或扫描文件。YAML、SQLite、ECS 不提供分布式原子提交。远程启动仅引用 workspaceID + expected revision，由 daemon 读取校验编译定义，不接受任意数据库路径；同机 CLI 使用相同规范根。当前接口已接通，具体安全行为由存储与生命周期实现承担，各自验证范围独立。

## 领域推理请求分层（已实现）

共享层单独以 `inference_request_types.rs` 保存纯值 `DomainRequest { messages: Vec<AgentMessage>, tools: Vec<ToolDefinition>, config: DomainRequestConfig }`。`DomainRequestConfig` 只包含上游模型名、temperature、max_output_tokens、top_p、stop；不包含厂商扩展、URL、密钥、HTTP Client、Entity、取消或回复通道。Default 是空请求，不是有效的默认模型路由。

`InferenceRequest` 继续表示带运行期关联和 oneshot 的执行事件，不能与纯值请求混为一体。API 格式、三种具体请求、`ApiRequest`、厂商处理器及转换错误只在 `inference_plugin::types` 定义；共享层不依赖推理插件。可失败转换采用 `TryFrom` / `try_into()?`，不为方便而丢弃消息信息。三个具体格式转换及vendor处理已经实现；共享层不包含具体HTTP格式。

## reasoning replay（已实现）

Assistant包含可选 `replay: Option<serde_json::Value>`；旧消息缺字段默认None。共享层将其视为不透明纯JSON：`{ "api_format": "messages" | "responses", "items": [...] }`。Messages保存signed thinking的文本/signature及redacted_thinking.data；Responses保存reasoning item的id、summary和可用的content/encrypted_content。DTO、持久MESSAGE和显式历史保留replay，不把它拼入正文，也不以可显示reasoning代替签名或加密元数据。格式边界拒绝不兼容/无效replay及无法无损表达的历史；Responses使用store=false及include reasoning.encrypted_content实现完整本地无状态重放。User和ToolCall结果均支持独立images数组，content保持String。

## 共享消息与执行边界

`AgentMessage` 是纯消息值，不是带 Entity 的路由事件；包含 Inject、System、User、Assistant、ToolCall 结果及 Error。`ToolCall` 输入只存在于 Assistant.tool_calls。资源可见性由 Agent 的显式导入绑定与 MESSAGE/TOOL/SANDBOX typed arrays 决定，不恢复旧 AgentFailure、邮箱或可见性集合。

## 用户轮次中止契约（已实现）

无 `get_turn`。Agent采用 Creating / Idle / Working / Stopping，对外同步为 `creating/idle/working/stopping`，同时发布 turn 与 status。启动成功进入 Idle、turn=None；`start_turn` 仅允许 Idle，原子创建唯一 turn 并置 Working，不覆盖已有轮次。`AbortAgentTurn` 验证完整Agent实例、workspace、Driver实例及expected_turn_id；受理后立即清空 turn，但保持 Working，只取消匹配活动的独立 `turn_cancel`，不关闭根作用域。正常结果链的 `finish_turn` 核对当前Driver关联与真实清理完成后才置 Idle；Idle重复完成幂等，Working且turn=None并不代表完成，迟到旧完成不能覆盖新轮次。用户轮次中止不等于根 Agent shutdown、owner abort、timeout 或 receiver closed。

Assistant 的 `stop_reason: Option<StopReason>` 使用serde默认值兼容旧文件缺字段为None；prompt等非模型生成消息使用None，每个实际推理输出使用Some，覆盖所有推理原因，包含 `StopReason::Cancelled`（序列化 `cancelled`）。兼容保留 `InferenceResponse.stop_reason`，由同一权威映射构造，必须与Assistant中的Some值相同。用户轮次中止且真实清理成功返回 Ok assistant：保留已接受的部分 `content` 与 `reasoning`（二者皆空时均为 `None`，不填充提示正文），`replay=None`、`tool_calls=[]`、`usage=None`、`stop_reason=Some(Cancelled)`。

取消工具且真实清理成功返回 Ok ToolCall 结果：`failed=true`、`content="Tool call aborted."`，保留调用 ID、名称及已知 resource_id；未启动项同用该正文。批次每个调用均有一个结果，保留已完成结果，不虚构部分成功，不撤销已发生副作用。`CleanupFailed` 仍为 Err，保留活动，不宣称空闲。

AbortAgentTurn控制系统注册在 `RuntimePlugin::PRE_UPDATE`，先于UPDATE中的 MCL 实际执行与 inference/tool admission；自定义调度必须保留这个先后关系。共享中止事件允许多个消费者独立观察，不靠 take reply 独占通知；实际受理重新核对Working及非空匹配的请求 turn，turn=None 或已中止旧轮次返回正常取消结果，不启动执行。

历史仅由显式 MCL `history_append` 保存。Assistant stop_reason 存在历史 payload 元数据，不混入正文；cancelled assistant 可入历史但不可进入 infer 上下文（包括 Inject 嵌套）。领域请求、结果与错误保持各自 oneshot/共享事件关联，不恢复 AgentFailure 路由。

## Workspace 成员启动失败隔离契约

Workspace 自身定义校验、创建或执行线程创建失败才返回 StartWorkspaceResult::Err；创建前失败不占用新地址，重复地址保留旧实例并报告其 ID、状态、成功成员数及失败成员数，不覆盖或自动附着。初始驱动和成员请求全部结束后，即使全部成员失败也进入 Running 并返回 Ok，成员失败不得关闭提交门。

Workspace 保存 member_failures: BTreeMap<ResourceID, WorkspaceMemberFailure>；失败值包含 driver 与 error，工作区身份由所属组件提供。这是持久到该工作区停止的失败诊断，不是重复 Agent 生命周期表。每个失败成员产生 WorkspaceMemberStartFailed，携带 workspace Entity、workspace_id、agent、driver、error；共享驱动失败按成员展开，动态创建失败同样记录并通知，成功加入清除该成员旧失败。Stopping 中已受理动态创建失败只结束等待，不污染健康工作区的停止结果。

WorkspaceInfoDto 新增 member_failures: Vec<WorkspaceMemberFailureDto>，各项 agent、driver 为 ResourceIDDto，error 为 String；serde 缺省为空、空列表省略。workspace.started、state.sync、state.query 均投影该唯一失败源。DTO 按配置 logs 目标逐成员发送包含工作区、成员、驱动和原始错误的日志。目标移除manager专属路由及manager unavailable错误；请求应显式定位普通成员，无隐式切换。客户端收到带失败的 workspace.started 仍保持连接。

## 用户及工具结果图像契约

User与ToolCall执行结果保持 `content: String`，增加 `images: Vec<UserImage>`；缺少images反序列化为 `[]`，空数组序列化省略。`UserImage { media_type: String, data: String }` 中data是规范标准base64（含必要padding），不是data URL；允许image/png、image/jpeg、image/gif、image/webp。每条User或ToolCall结果最多8张，每张解码后最多5 MiB，总计最多20 MiB。共享公开函数 `validate_user_images(images: &[UserImage]) -> Result<(), String>` 在解码前检查数量、编码长度及解码长度上界，再严格验证base64及与MIME一致的PNG/JPEG/GIF/WebP魔数，不执行完整图像解码。错误只含索引、限制和原因，不回显base64。公开serde辅助函数 `deserialize_user_images` 供共享消息及边界复用；领域User和ToolCall结果反序列化也调用共享验证，覆盖直接MCL/Lua/持久化入站；Protocol入站递归转换调用共享验证，出站原样保留images；MESSAGE、Lua及历史payload保留图像，不将base64拼入正文。协议原有1 MiB非图像消息限制独立保留，图像编码单独按上述限制计量。
