# DtoPlugin

`DtoPlugin` 是 WebSocket DTO 转换层。它消费 ServerPlugin 的 `WebSocketMessageReceived`，解析统一的
`{ type, id, message }` 信封，调用对应 DTO 的 `into_domain` 方法，并直接发送领域事件。

入站路由：

```text
connection.register -> RegisterConnection
workspace.start      -> StartWorkspace
agent.message        -> RouteAgentMessage
```

它还收集允许发送给外部的 `StartWorkspaceResult`、`AgentMessage` 和 `AgentFailure`，调用 Protocol
定义的 DTO 转换，构造 `ServerMessage` 并包装为 `WebSocketMessageSend`。

它不创建 ECS Entity，不决定 manager，不执行消息意图，也不检查资源权限。WorkspacePlugin 负责逻辑
Workspace/Agent 路由，AgentPlugin 只处理已经携带 Entity 的消息。

后端通过 `WebSocketMessageSend` 请求发送 `ServerMessage`。DtoPlugin 负责序列化和根据
`Broadcast`、`Type`、`Name` 筛选连接。
