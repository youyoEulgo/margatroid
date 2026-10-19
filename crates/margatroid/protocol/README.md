# Margatroid Protocol

## 当前协议状态

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

## 信封、身份与关联

客户端统一发送 `{type,id,message}`。id 必须非空且最多 512 UTF-8 字节；除 connection.register 外先完成连接注册。DtoPlugin 将 source 绑定到真实 Client ResourceID，不接受客户端提供 source、Entity、句柄、数据库路径或 Lua 源码。Client 的连接与完整 Entity generation 共同决定响应归属。

资源地址使用 `ResourceIdDto` 字符串（兼容别名 ResourceIDDto），按共享 ResourceID 规范化；Driver 为 `driver:scope/name:tag`，没有 agent-driver 别名。Workspace/Agent 不存在 manager、默认首成员或 null 目标回退。

- 工作区注册操作使用 Workspace ResourceID。
- MCL、历史、中止与停止的 workspace 使用 `{id,name,project_root}`，必须与当前实例相符。
- MCL/history/abort 的 agent 必须是显式规范 Agent ID，且属于目标工作区；缺失或 null 返回可操作错误。
- `agent_instance` 是当前完整 Entity generation 的不透明输出标识，只用于拒绝迟到结果，不授予入站寻址权。

除注册与既有 MCL 关联规则外，目录、变更、启动、停止、中止、目录浏览和 Driver 清单请求均使用独立内部 operation_id；网络 id 只在回执时还原。两个连接可复用同一网络 id，不能串回执；同一 Client 尚未完成的重复 id 拒绝。断连清除网络关联，不自动撤销已受理的领域操作，也不重建迟到关联。

## 工作区目录与变更

| type | message | 成功回执 |
| --- | --- | --- |
| connection.register | `{client_type,name?}` | connection.registered |
| workspace.catalog.query | `{}` | workspace.catalog |
| workspace.catalog.refresh | `{}` | workspace.catalog |
| workspace.start | `{workspace,expected_revision}` | workspace.started |
| workspace.stop | `{workspace: WorkspaceReferenceDto}` | workspace.stopped |
| workspace.member.add | `{workspace,expected_revision,agent:{name,driver,home_root?}}` | workspace.member.result |
| workspace.member.remove | `{workspace,expected_revision,agent: ResourceIdDto}` | workspace.member.result |
| workspace.create | `{name,project_root}` | workspace.member.result |
| workspace.rename | `{workspace,expected_revision,new_name}` | workspace.member.result |
| workspace.delete | `{workspace,expected_revision}` | workspace.member.result |
| workspace.member.rename | `{workspace,expected_revision,agent: ResourceIdDto,new_name}` | workspace.member.result |

所有 wire 字段使用上述 snake_case，特别是 `expected_revision`；不引入 expectedRevision。create 没有客户端修订号，服务端以“目标名称/目录不存在”为创建前提。其他定义修改核对最新源内容修订，冲突后刷新，不覆盖。变更及启动 DTO 拒绝未声明字段，不能在请求内附带 arbitrary definition/YAML/history_path。

`workspace.catalog {id,entries}` 返回全部注册项，包括 stopped、空成员及无效配置；query 读取最近目录快照，refresh 经存储服务重新扫描，不自动运行实例。

`WorkspaceCatalogEntryDto`：

```text
id: ResourceIdDto
name: String
path: String                         注册定义位置
revision: String                     源内容版本；坏/缺失源不能伪造可用版本
definition?: WorkspaceDefinitionDto
error?: String
lifecycle: stopped | starting | running | stopping | failed
```

有效定义为 `{id,name,project_root,agents:[{id,name,driver,home_root}]}`，无 manager、history_path 或 memory_path。定义有效性与运行状态分开；配置失效或整个 home 消失时仍须保留尚存 live 实例的诊断。运行事实不能从 SQLite/历史是否存在推断。

`workspace.member.result {id,entry,error?}` 是统一定义变更结果：

- 保存/重命名成功：entry 为更新的权威目录项，省略 error。
- 失败：entry=null 且有结构化 error；错误需区分持久失败、停止失败及已停止但配置未提交的部分完成事实。
- **删除工作区成功：entry=null，省略 error。** 不能把删除后查不到条目合成为失败。客户端随后以权威 catalog 确认移除。
- 新成员定义已保存但启动失败不反转持久成功；运行失败由 member_failures、Agent 状态与日志表达。

共享 WorkspaceDeletion 是内部可恢复移除结果，不要求独立删除 wire，也不暴露内部回收文件位置。Workspace 重命名/删除要求无 live 实例；成员重命名/删除须先完成必要的 stop/drain。真正持久迁移与回收由生命周期和存储能力负责，不由 Protocol 转换执行。

## 新建工作区的项目目录选择

项目路径属于 daemon 所在机器，不能使用浏览器上传目录的名称推测主机绝对路径。

```text
workspace.paths.query
    message: { path?: String, cursor?: String }
workspace.paths
    { id, path: String, parent: String|null,
      directories: [{ name: String, path: String }], next_cursor?: String }
```

path 省略时从 daemon HOME 开始；给定时必须为可读的绝对目录，后端返回规范化绝对 path、父目录及子目录。只列目录，不读文件内容、不创建目录；按名称稳定排序并有界分页（每页最多 200 项）。cursor 为不透明分页位置，绑定规范路径和目录页版本；路径变化/失效时返回错误，不能重用旧 cursor 拼接另一目录。子目录枚举跳过符号链接；手动指定的绝对路径通过 canonicalize，可解析符号链接并返回目标规范目录。每次最多检查 20,000 个目录项，超限返回错误。无法读取、目录消失、无效 cursor 均返回关联 request.failed。

入站由共享 QueryProjectDirectories 及结果关联，实际 I/O 由组合根注入的只读 ProjectDirectoryBrowser 能力在 blocking worker 执行。Protocol 只做 DTO 和纯转换。该接口不是认证机制；注册与入口策略不代替主机权限。选择后 workspace.create 仍重新校验现存项目根，不信任旧浏览结果。

## 可用 image / AgentDriver 清单

image 是界面用词，对应现有 AgentDriver 包，不是模型路由或运行 Agent。

```text
driver.catalog.query        message: {}
driver.catalog.refresh      message: {}
driver.catalog              { id, drivers: [ResourceIdDto], errors: [{path,message}] }
```

query 返回最近已验证快照，refresh 通过 loader 的受检扫描重新检查安装包；启动时同样建立清单。包括从未挂载到任何 Workspace 的有效包，不以 ResourceSet 挂载历史充当安装目录。只对外公开规范 Driver ID 及包错误诊断，不公开源码、依赖、模型设置或凭据。无可用包返回空列表，不假造默认驱动。真正加载和新增成员时仍重新验证所选资源。

`state.sync.state.drivers` 同步有效地址数组，缺字段按 `[]`、空列表可省略；刷新失败保留最后确认快照并返回本次错误，不宣称已成功检查最新文件。

## 状态、历史与流

`state.sync {state}` 的 state 包含 `catalog,workspaces,agents,histories,drivers`。catalog 是全部注册项，workspaces/agents 是真实运行实例。WorkspaceInfoDto 为 `{id,name,project_root,agents,member_failures?}`；member_failures 各项 `{agent,driver,error}`，空列表可省略。一个或全部成员启动失败不使已经建立的 Workspace 伪报未启动。

AgentStateDto 包含 `workspace,agent,agent_instance,status,turn,sandbox,queued_commands,pending_command`，可含 context_window_tokens 和 error。status 为 creating/idle/working/stopping，不增加虚假的 failed Agent 生命周期。Driver/清理错误通过 error 表达，不能由 turn=null 推断 Idle。

`history.query {workspace,agent}` 返回 `history.result {id,history}`，历史当前从目标 live Agent 的 history 句柄读取，不承诺停止工作区的离线历史查询已存在。HistoryMessageDto 包含 key、1-based list 顺序 sequence、turn、kind、content、payload、created_at_ms。正文与元数据分开；旧时间 0 表示未知，不以迁移时间或当前时间回填。注册信息和历史存在不证明当前运行。

流使用 agent.message.delta / agent.message.reasoning_delta，字段 `{id,agent,agent_instance,sequence,content}`；类别由 type 表达，无额外 kind 字段。id 是推理请求 ID，不是 turn。agent.message.finished `{id,agent,agent_instance,error}` 标记流终态；流和 finished 只服务显示，领域 oneshot/MCL 结果才是操作完成权威。

## MCL 与消息值

`mcl.command {workspace,agent,command,arguments}` 一次一条指令。每个 arguments 元素必须为数组，按顺序对应问号；无问号用空数组。id 在外层信封，不是 MclCommandDto 内层字段。source 来自真实注册 Client。

`mcl.command_result {id,source,target,result,history_error}` 只回发起连接；result 保留 `{"Ok":...}` / `{"Err":{kind,message}}` 和 MclValue 标签。GET 出站 Message/Tool/Sandbox 为安全投影：TOOL 只有 resource_id/alias/description/input_schema，SANDBOX 只有 resource_id，不泄漏脚本路径或策略。出站对象不能绕过入站的导入绑定和类型检查。

AgentMessageDto 是共享消息的递归投影：System、Inject、User、Assistant、ToolCall 结果、Error。Assistant 输入调用元素 ToolCall 与执行结果消息 ToolCall 不混同；允许序列化消息不代表外部请求可伪造执行完成。AgentMessageDto 转换限制深度 32、节点 4096、非图片序列化大小 1 MiB；这不等同于整个 MCL 信封的入口上限。DTO 入口另有总 32 MiB 限制，直接 MCL 手工消息仍要通过共享消息校验。

User/ToolCall 保持 content:String，images 是独立 `{media_type,data}` 数组。data 为含必要 padding 的标准 base64，不是 data URL；允许 PNG/JPEG/GIF/WebP，每条最多 8 张、单张解码 5 MiB、合计 20 MiB。共享 validate_user_images/deserialize_user_images 在解码前检查预算，随后检查 base64 与 MIME 魔数，不完整解码图片；错误不回显数据。空 images 省略、旧消息缺字段为[]。历史与 Lua 保留 images，不把数据混入正文。真实工具结果来自执行 oneshot。

replay 为 Assistant 可选不透明 JSON `{api_format,items}`，缺字段 None，往返与显式历史原样保留。Messages 保留签名/遮蔽 thinking，Responses 保留 reasoning ID、summary、content 和加密值；Protocol 不伪造、不解释。格式边界检查兼容性；Responses 每次本地完整上下文、store=false，不依赖远端会话。

## 中止契约

agent.turn.abort `{workspace,agent,turn_id}` 为独立控制请求，完整实例/预期 turn 校验后触发匹配 turn_cancel，不排在 MCL FIFO 中。abort_result 回显 id/workspace/agent/turn_id 及 Result；Ok 只证明受理，不证明清理结束。Working 中清 turn 后仍是 Working，直到真实清理及 Driver finish_turn 才 Idle。重复中止只按当前实例保留的关联幂等，不取消随后新轮次。

组合根必须把 Agent 控制系统放在域操作 admission 之前：daemon 使用 PRE_UPDATE 中止、UPDATE 推理/MCL/工具；自定义调度不能假定各插件默认计划自动保证这一点。

用户取消推理在本地 worker abort+join 后返回累计 reasoning/content 的 cancelled Assistant；无输出保持空值，无工具调用、replay 或伪造 usage。普通结束原因、根关闭、owner abort、timeout、receiver closed 和 CleanupFailed 仍须区分。工具取消保留调用 ID/顺序与已知资源，failed=true，正文 Tool call aborted.；已完成项保留真实结果。历史只由显式 history_append 写入；取消 Assistant 可入历史但不可入推理上下文（含嵌套 Inject），工具结果仍按规则入上下文。

前端点击立即冻结显示只是本地显示屏障，不是清理事实；最终权威历史可补齐服务端断流前接受的部分输出。cancelled 不染红、不添加标签、不补 Turn aborted.。本地断连不保证供应商停止远端计算或计费。

## 错误与验证边界

ProtocolErrorDto `{kind,message}` 保留请求关联，message 最多 512 UTF-8 字节且不含凭据或图片正文。无效/未注册/目标不存在/重复/版本冲突等不能静默丢弃。结构错误有可用 id 则 request.failed；无法关联以协议关闭反馈。当前 scoped 验证覆盖对应路由与请求关联；持续回归边界包括：同网络 id 跨客户端隔离、重复在途拒绝、断连不串回执、空 managerless 工作区、显式 Agent、超过 1 MiB 的图片请求、目录/驱动/路径选择、所有变更结果及失败/未知提交语义。
