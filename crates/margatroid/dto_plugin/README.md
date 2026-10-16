# DtoPlugin

DtoPlugin 是 WebSocket 传输与共享领域事件之间的转换层。协议值由 Protocol 提供，领域组件与事件由共享 types 提供；不依赖 Agent、MCL、Workspace、Client 或 Memory 插件实现。

## 用户轮次中止契约（已实现）

独立控制请求转换为共享 AbortAgentTurn；网络层仅保存发起连接与完整领域关联，不恢复旧邮箱或执行 pending 轮询。无 `get_turn`。

- 请求：`{ "type":"agent.turn.abort", "id":"abort-1", "message":{ "workspace":{id,name,project_root}, "agent":null, "turn_id":"turn-1" } }`。`id` 为网络请求关联，`turn_id` 为必填非空的预期活动轮次；agent 为 null 时选择 manager，其余身份规则与 MCL 相同。
- 回执：`agent.turn.abort_result { id, workspace, agent, turn_id, result }`；result 使用 `{"Ok":null}` / `{"Err":{kind,message}}`。Ok 仅确认中止已受理或该关联轮次已中止，不证明执行 cleanup 已结束；失败保留请求 id。仅回复发起连接，source 来自真实注册 Client。
- 入口受检解析 Workspace 与当前成员的完整实例，实际处理再次核对 turn_id；旧/不匹配轮次不得取消新轮次。对同一已中止轮次（含 Working 且 turn=None）重复请求幂等成功，仅保留必要已中止关联，不恢复全局 pending 表；未识别轮次报关联错误。
- 状态为 `creating/idle/working/stopping`。中止立即清空 turn，真实清理期间保持 Working，正常 finish 收口才 Idle；状态同步同时发布 turn 与 status。前端据 status 显示运行态、据 turn 关联中止，不能把回执或 turn=None 当作 Idle。
- AgentPlugin 将 abort_agent_turn_system 固定安装到 RuntimePlugin::PRE_UPDATE，先于 UPDATE 中的 MCL 实际执行及 inference/tool admission。共享事件允许各消费者独立观察，不靠 take reply 独占中止通知；执行前再次核对请求轮次，turn=None 或已中止旧轮次不启动推理/工具而返回正常取消结果。
- Assistant 新增 stop_reason，覆盖全部推理原因，新增 `StopReason::Cancelled` 序列化 `cancelled`。兼容保留 `InferenceResponse.stop_reason`，单一权威映射生成，两处始终相同。用户轮次中止且真实清理成功返回 Ok assistant：保留已接受的部分 `content` 与 `reasoning`（二者皆空则保持空值，不填充取消文案），`replay=None`、`tool_calls=[]`、`usage=None`、`stop_reason=Cancelled`。
- 取消工具且清理成功返回 Ok ToolCall：`failed=true`、`content="Tool call aborted."`，保留调用 ID、名称及已知 resource_id。批次保留已完成真实结果，其余每个调用（包括未启动项）各有一个原序结果；不虚构部分执行成功，不撤销已发生副作用。
- `CleanupFailed` 仍为 Err，保留活动，不伪造取消或 Idle。用户轮次中止不同于根 Agent shutdown、owner abort、timeout、receiver closed，不统一转换这些原因。
- 历史只经显式 history_append 保存；Assistant stop_reason 存入 payload 元数据而非 content。cancelled assistant 可入历史，不得进入 infer 上下文（包括 Inject 嵌套）。

## 当前路由

| 入站 type | 行为 |
| --- | --- |
| `connection.register` | 发送 ServerPlugin `RegisterConnection` |
| `workspace.start` | 转换共享 `StartWorkspace` 并保存网络回复关联 |
| `workspace.stop` | 受检解析完整 Workspace 后调用共享 World 扩展 |
| `agent.turn.abort` | 解析当前成员完整 VM 实例，发送共享 AbortAgentTurn 独立控制事件 |
| `mcl.command` | 解析当前成员或 manager，绑定真实注册 Client.resource_id，发送共享 MclCommand |
| `state.query` | 只读状态快照，直达请求连接 |
| `history.query` | 从当前 Agent.history.list 读取历史，直达请求连接 |

除注册外，未注册连接返回关联请求 ID 的 `request.failed`，不延后受理。注册成功回执仅在共享 Client 实体存在且身份匹配后发送。
入站 JSON 总上限为 32 MiB，与 WebSocket 图片传输边界一致；图片请求不再被旧 1 MiB 检查静默丢弃。业务层继续校验图片数量、解码大小和格式。超限关闭连接并记录字节数，不记录图片正文；结构错误有合法请求 ID 时返回 `request.failed`，无法关联时以协议错误关闭，客户端不应无反馈等待。

MCL Result 按 id/source/target 匹配待回复记录，仅向发起连接发送。回复前再次检查完整 Client Entity generation 与 source，迟到结果不会发送给复用身份的新实例。Workspace 请求使用内部唯一 operation_id 关联领域结果，网络回复还原原始 id，防止不同 Client 的相同网络请求 ID 相互混淆。

断开连接时网络层待回复记录被清除；同时按连接存活和完整 Client Entity 核对并清除陈旧记录，包含 MCL、Workspace start/stop、中止和历史快照关联。此清除只释放网络回复关联，不自动取消已提交的领域操作。清除后的迟到结果被忽略，不能重建回复记录。

## 出站和状态

`WebSocketMessageSend` 仅重新导出共享 types::dto_events 事件，其字段为共享 target 和 JSON Value，不携带 Protocol 的 ServerMessage。调用方先将 ServerMessage 序列化为 Value；DTO 按 Broadcast/Type/Name 筛选实际连接并发送 Text。

状态快照从 Workspace、Agent 与 Agent.history 读取，仅在状态内容或目标连接集合变化时发送，避免事件驱动 Runtime 的自发循环。历史 payload 保留结构化消息与 usage。协议投影 sequence 为列表顺序的 1-based 编号；created_at_ms 保留共享 HistoryRecord 的创建时间，旧记录的 0 表示未知。

日志订阅使用 TracingStream，经共享 JSON 事件发送到配置的 logs 目标。日志自身发送失败不再生成日志，订阅滞后不写回 tracing，避免递归。

## 边界与已归档行为

插件不创建领域 Entity、不执行 MCL、不操作 Driver、不负责鉴权或资源权限。身份绑定仅保证请求来源与连接一致，不等同完整认证。

旧 agent.message 邮箱、旧 agent.turn.abort 查表实现、visibility、Workflow 与 pending oneshot 轮询表已移除；上文 agent.turn.abort 使用新的共享控制事件，不恢复旧实现。旧 Memory/HistoryMessage、默认可见性、AgentTokenUsage 副本与消息广播报告不是当前接口。当前 pending 是网络请求回复关联，由共享结果事件驱动，不是 Agent 的执行 pending。

中止网络关联使用内部唯一 operation_id，并核对 agent Entity、resource_id、workspace、完整 VM instance 与 expected_turn_id；只回复原 Client 实例与连接，断连清除关联但不撤回已受理中止。Working 期间拒绝 INJECT ? 中递归包含 user 的消息参数；工具可见性、sandbox 等非用户数组不被此守卫误拦截。

## Workspace 成员启动失败隔离契约

外部 `workspace.start` API 必须在成员定义中声明 manager，因为出站 `WorkspaceInfoDto.manager` 是必填资源地址。DTO 在发送 `StartWorkspace`、保存网络关联或创建领域实体之前验证定义可投影；空成员或缺少 manager 定义返回可操作的 `InvalidRequest`，不占用工作区地址。此入口限制不改变领域内部对空 Workspace 的支持，也不改变 wire 字段。已声明 manager 但全部成员启动失败仍可正常投影为 `workspace.started`。

Workspace 自身定义校验、创建或执行线程创建失败才返回 StartWorkspaceResult::Err；创建前失败不占用新地址，重复地址保留旧实例并报告其 ID、状态、成功成员数及失败成员数，不覆盖或自动附着。初始驱动和成员请求全部结束后，即使全部成员失败也进入 Running 并返回 Ok，成员失败不得关闭提交门。

Workspace 保存 member_failures: BTreeMap<ResourceID, WorkspaceMemberFailure>；失败值包含 driver 与 error，工作区身份由所属组件提供。这是持久到该工作区停止的失败诊断，不是重复 Agent 生命周期表。每个失败成员产生 WorkspaceMemberStartFailed，携带 workspace Entity、workspace_id、agent、driver、error；共享驱动失败按成员展开，动态创建失败同样记录并通知，成功加入清除该成员旧失败。Stopping 中已受理动态创建失败只结束等待，不污染健康工作区的停止结果。

WorkspaceInfoDto 新增 member_failures: Vec<WorkspaceMemberFailureDto>，各项 agent、driver 为 ResourceIDDto，error 为 String；serde 缺省为空、空列表省略。workspace.started、state.sync、state.query 均投影该唯一失败源。DTO 按配置 logs 目标逐成员发送包含工作区、成员、驱动和原始错误的日志。manager 不可用时路由返回明确 manager unavailable 诊断，不暗中切换成员。客户端收到带失败的 workspace.started 仍保持连接。
