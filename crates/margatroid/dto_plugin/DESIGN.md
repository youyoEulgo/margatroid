# DtoPlugin

## 当前状态与范围

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

DtoPlugin 是 WebSocket 与共享领域事件之间的边界，不执行 MCL、SQLite、YAML、磁盘扫描或数据库迁移，不依赖业务插件实现。Protocol 定义传输值及纯转换，共享 types 定义领域事件与能力，组合根安装真实实现。

# lib

安装 /ws 事件路由、结果收集与普通出站投影，持有网络响应映射及 BackendStateReportCache，订阅 TracingStream。要求 Runtime/AsyncRuntime、ServerPlugin、ConfigPlugin 和日志流已由组合根安装；不在本插件启动另一个服务器或构建工作区目录。

# system

```text
dto_route_system
    克隆本帧WebSocketMessageReceived，逐项handle_inbound_message
    清理断连/旧Client generation的网络pending
    消费中止与MCL完成结果并匹配原请求
    发送已准备好的WebSocketMessageSend
collect_external_events_system
    处理server/注册/目录/变更/工作区/路径/Driver清单结果
    发布实际成员失败日志
    根据元数据/目录/接收者变化及必要dirty信号准备权威状态
```

这些路由和收口系统当前已实现。事件读取非独占、通常下一帧可见；关联结果只由本插件自有pending一次收口，不能把共享event_reader当消费队列。

# handler

## 入站保护

Text总上限32MiB，与WebSocket接收一致。超限记录字节数、不含载荷，关闭1009；解析/结构错误有非空有界id则关联request.failed，无法关联则关闭1007。显式Agent缺失/null给出修复提示，不回显图片/私有字段。没有无反馈的1MiB早退。

验证信封id、真实注册Client；除注册外未注册返回关联错误，不延后重试。Source绑定当前Client.resource_id。重复检测覆盖aborts/commands/workspaces/snapshots/catalog及新路径/Driver查询关联，同一Client在途id冲突拒绝；不同Client同网络id允许且不能串回执。

## 路由表

| 请求 | 领域动作 | 回执 |
| --- | --- | --- |
| connection.register | RegisterConnection | connection.registered / register_failed |
| workspace.catalog.query / refresh | QueryWorkspaceCatalog / RefreshWorkspaceCatalog | workspace.catalog |
| workspace.start | StartRegisteredWorkspace，仅ID+expected_revision | workspace.started / start_failed |
| workspace.stop | 受检实例StopWorkspace | workspace.stopped / stop_failed |
| workspace.member.add | AddWorkspaceMember，WorkspaceMemberSpec | workspace.member.result |
| workspace.member.remove | RemoveWorkspaceMember | workspace.member.result |
| workspace.create | CreateWorkspace `{name,project_root}` | workspace.member.result |
| workspace.rename | RenameWorkspace `{workspace,expected_revision,new_name}` | workspace.member.result |
| workspace.delete | DeleteWorkspace `{workspace,expected_revision}` | workspace.member.result |
| workspace.member.rename | RenameWorkspaceMember `{workspace,expected_revision,agent,new_name}` | workspace.member.result |
| workspace.paths.query | QueryProjectDirectories `{path?,cursor?}` | workspace.paths |
| driver.catalog.query / refresh | QueryAgentDriverCatalog / RefreshAgentDriverCatalog | driver.catalog |
| mcl.command | 受检显式成员MclCommand | mcl.command_result |
| agent.turn.abort | 完整实例+预期turn的AbortAgentTurn | agent.turn.abort_result |
| state.query | 只读元数据+异步历史快照 | state.sync |
| history.query | 受检live Agent句柄异步读取 | history.result |

路径/Driver查询与四个变更均已接通。精确wire以Protocol为唯一字段契约，统一expected_revision而非camelCase；create不接收revision，服务端执行目标不存在的创建检查。不得恢复manager预检、任意定义上传、数据库路径入站或null成员默认路由。

## 关联与结果收口

```text
非注册业务请求
    分配内部唯一operation_id（MCL保留现有id/source/target关联）
    记录网络id/connection_id/完整Client Entity/操作种类
    校验完成后保存网络关联，再发送共享领域事件
结果到达
    按内部operation_id及操作种类找到唯一pending
    只移除该pending；错误或无关结果不能释放其他请求
    再核对Client Entity generation和连接归属
    还原网络id，投影真实结果，然后只回复发起连接
```

- catalog query/refresh 使用结果携带的权威entries，叠加当前运行生命周期，不从旧缓存伪造成功。
- member.add/remove/create/rename/member.rename 成功带entry，error省略；持久失败entry=null且有error。
- **delete成功entry=null且无error**，不因目录查无该项制造错误，也不暴露内部回收路径；共享WorkspaceDeletion仅为领域内部事实。
- 新增或改名的持久提交成功后，运行成员启动失败由member_failures/Agent错误和日志表达，不反转为“配置没保存”。停止成功但保存失败保留真实已停止/仍配置/需恢复等信息。
- registered start只消费StartRegisteredWorkspaceResult，成功从实际Entity投影WorkspaceInfoDto保留member_failures；stop回执保留原WorkspaceReference，不能把可空的start reference当成功stop对象。
- 断连只清网络关联，不撤回已提交领域操作；迟到结果忽略，不能投递到复用连接或新Client generation。

## 路径与Driver服务边界

ProjectDirectoryBrowser由daemon注入：blocking worker对已存在目录执行规范化、权限检查、子目录排序/每页200/cursor校验，不读取文件正文、不mkdir。DTO只构造QueryProjectDirectories及ProjectDirectoryPage投影，默认路径由服务端HOME决定。具体request/reply字段见Protocol；选择旧结果不替代create时重新校验。

Driver清单由loader验证安装包后发布DriverCatalog，包含尚未挂载包，canonical driver:*地址。query读快照，refresh在worker重验并返回有效列表/错误；state.sync.drivers同步快照。DTO不能从ResourceSet挂载记录拼出完整可用目录，不在主线程扫描。只输出安全ID和包诊断，无源码/密钥/模型配置。

## 中止与成员定向

MCL/history/abort必须显式Agent ID。resolve_workspace/resolve_agent核对资源、项目根、归属和完整实例。Working期间手工INJECT递归包含User的参数拒绝，非用户资源数组不误拦；别名/selector不是开放用户发送接口。

中止保存完整Agent/Workspace/DriverLuaVmInstance/expected_turn_id与内部op关联；匹配原结果后发abort_result，Ok仅表示受理。Token取消、真实HTTP/SSE或工具清理由对应业务执行层负责。不能以回执、空turn或前端显示屏障证明Idle。

## 状态与历史

主线程只捕获WorkspaceCatalog缓存、DriverCatalog、真实Workspace/Agent元数据及History句柄。历史从blocking worker读，具名DtoHistorySnapshotCompleted唤醒收口；Protocol只消费提供的记录。historical query当前依赖live Agent，不偷偷增加离线数据库访问。

准备state包含catalog/workspaces/agents/histories/drivers。注册定义有效性与运行状态独立；无效源和消失home仍保留live实例诊断。停机/DB文件存在/历史非空不能伪造运行状态。空成员Workspace可投影，不做manager存在性预检。

BackendStateReportCache追踪prepared_state、实际state、pending、dirty和接收者。元数据/目录/Driver目录变化、已提交MCL可能改变历史或新连接需同步时准备；完全相同内容和接收者不重复发送，不由自己发出的state.sync触发无限循环。异步读取期间实例被替换则拒绝旧结果并重排，而不是把旧历史绑定新generation。不能只检查agents而忽略catalog或drivers变化。

历史content是文本，payload保留类型、images、replay、usage、stop_reason、工具关联和审计；不将base64展示成日志。sequence按返回list顺序1-based，created_at_ms用原记录值、旧0表示未知。显式history_append是消息历史唯一入口，推理/工具完成不自动追加。

## 日志与出站

只剩outbound.logs与outbound.backend_state。logs控制tracing转发及领域诊断，backend_state控制主动state.sync；请求回执直达原连接，不受广播配置拦截。推理增量当前由推理层固定选择webui快照，不虚称已按被删除配置字段路由。

TracingStream是有界临时广播，无持久历史/断线补发承诺；lagged跳过，日志发送失败不再生成日志以免递归。普通WebSocketMessageSend根据broadcast/type/name解析连接并构造发送终端，不解析业务或操作数据库。

# events

WebSocketMessageSend重新导出共享dto_events，只含target+JSON Value。目录、生命周期、Driver和路径服务的共享事件由types承载；DTO不重复定义领域执行状态机。异步历史完成用共享具名事件，不靠pending轮询判断领域操作完成。

# types

```text
PendingMclCommandResponses
    aborts/commands/workspaces/snapshots/catalog
    discovery 表保存路径页与 Driver 查询/刷新关联，并按操作种类检查
PendingCatalogResponse
    operation_id/id/connection_id/client/kind
    kind精确区分Query/Refresh/MemberAdd/MemberRemove/Create/Rename/Delete/MemberRename
    不能将Delete结果错误解释成有entry的查询
PendingWorkspaceResponse
    start关联内部ID；stop保留Some(reference)，start可为None
BackendStateReportCache
    state/prepared_state/pending/dirty/recipients/last_error
```

网络pending不是Agent执行pending，也不是持久目录权威。操作失败不能让错误pending覆盖已受理操作，完整Client generation为所有回复的必要条件。

# error

可关联失败保留原id，错误有界且无凭据/载荷。目录单项错误留entry.error，不隐去其他条目；目录全量/路径/Driver查询失败用request.failed。变更错误用member.result.error，运行失败另投影。提交未知和清理失败不伪造成功/回滚/Idle。

## 验证与范围

当前 DTO/Protocol scoped 验证合计 34 passed，覆盖恢复的 catalog、registered-start、成员与 CRUD/discovery 路由、精确网络 ID、generation 和错误关联。领域存储迁移与真实浏览器交互另有独立验证，不能由 DTO 测试替代。
