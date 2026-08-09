# DtoPlugin

`DtoPlugin` 是 WebSocket DTO 转换层。它消费 ServerPlugin 的 `WebSocketMessageReceived`，解析统一的
`{ type, id, message }` 信封，调用对应 DTO 的 `into_domain` 方法，并直接发送领域事件。

入站路由：

```text
connection.register -> RegisterConnection
workspace.start      -> StartWorkspace
workspace.stop       -> StopWorkspaceByReference
agent.message        -> RouteAgentMessage
```

它还收集允许发送给外部的 `StartWorkspaceResult`、`StopWorkspaceByReferenceResult`、`AgentMessage` 和 `AgentFailure`，调用 Protocol
定义的 DTO 转换，构造 `ServerMessage` 并包装为 `WebSocketMessageSend`。每个 Runtime tick 会从
Workspace、Agent 和 Memory 构造完整 `state.sync`，默认只发送给类型为 `webui` 的连接。

DtoPlugin 订阅 LogPlugin 提供的 `TracingStream`，将结构化日志转换为 `log` 消息并广播。因此安装前
必须先安装 AsyncRuntimePlugin、带 stream 的 LogPlugin 以及 ServerPlugin。Server 生命周期事件也由
DtoPlugin 的出站 system 写入结构化日志。它还记录 WebSocket 连接状态、Workspace 启停结果、可展示
Agent 消息方向和 Agent 失败；每帧状态同步和单个出站包不记日志，避免高频噪声和日志转发递归。

它不创建 ECS Entity，不决定 manager，不执行消息意图，也不检查资源权限。WorkspacePlugin 负责逻辑
Workspace/Agent 路由，AgentPlugin 只处理已经携带 Entity 的消息。

后端通过 `WebSocketMessageSend` 请求发送 `ServerMessage`。DtoPlugin 负责序列化和根据
`Broadcast`、`Type`、`Name` 筛选连接。`with_frontend_type` 可修改完整状态快照的目标连接类型。
