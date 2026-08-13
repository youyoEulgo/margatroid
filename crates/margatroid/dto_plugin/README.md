# DtoPlugin

`DtoPlugin` 是 WebSocket DTO 转换层。它消费 ServerPlugin 的 `WebSocketMessageReceived`，解析统一的
`{ type, id, message }` 信封，调用对应 DTO 的 `into_domain` 方法，并直接发送领域事件。

入站路由：

```text
connection.register -> RegisterConnection
workspace.start      -> StartWorkspace
workspace.stop       -> StopWorkspaceByReference
agent.message        -> RouteAgentMessage
agent.skill.load     -> RouteAgentSkill { Load }
agent.skill.unload   -> RouteAgentSkill { Unload }
agent.skill.unload_all -> RouteAgentSkill { UnloadAll }
```

它还收集允许发送给外部的 `StartWorkspaceResult`、`StopWorkspaceByReferenceResult`、`AgentMessage` 和 `AgentFailure`，调用 Protocol
定义的 DTO 转换，构造 `ServerMessage` 并包装为 `WebSocketMessageSend`。Workspace启动失败会同时
写错误日志并发送可按请求ID配对的`workspace.start_failed`终止回执。DtoPlugin 会从 Workspace、
Agent 和 Memory 构造完整 `state.sync`，但仅在状态内容或目标前端连接集合变化时发送，默认只发给类型为
`webui` 的连接。目标将改为统一读取主目录 `config.toml` 的 `backend_state` 字段。缓存避免出站事件持续唤醒 event-driven Runtime。

DtoPlugin 订阅 LogPlugin 提供的 `TracingStream`，将结构化日志转换为 `log` 消息并广播。因此安装前
必须先安装 AsyncRuntimePlugin、带 stream 的 LogPlugin 以及 ServerPlugin。Server 生命周期事件也由
DtoPlugin 的出站 system 写入结构化日志。它还记录 WebSocket 连接状态、Workspace 启停结果、可展示
Agent 消息方向和 Agent 失败；每帧状态同步和单个出站包不记日志，避免高频噪声和日志转发递归。
日志消息自身发送失败时不会再产生可转发警告，日志订阅滞后也不会向自身写回 tracing 事件。

它不创建 ECS Entity，不决定 manager，不执行消息意图，也不检查资源权限。WorkspacePlugin 负责逻辑
Workspace/Agent 路由，AgentPlugin 只处理已经携带 Entity 的消息。

后端通过 `WebSocketMessageSend` 请求发送 `ServerMessage`。DtoPlugin 负责序列化和根据
`Broadcast`、`Type`、`Name` 筛选连接，再构造 ServerPlugin 的 `WebSocketMessageSender` 并调用
`try_send`。日志、后端状态、完整成员消息和流式成员消息分别使用全局配置中的 `logs`、
`backend_state`、`member_messages` 和 `streaming_member_messages`；Workspace 启停结果及 Agent
失败或异常归入 `logs`。
