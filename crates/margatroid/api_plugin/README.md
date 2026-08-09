# ApiPlugin

`ApiPlugin` 是 WebSocket 传输层。它消费 ServerPlugin 的 `WebSocketMessageReceived`，解析统一的
`{ type, id, message }` 信封，调用对应 DTO 的 `into_domain` 方法，并直接发送领域事件。

入站路由：

```text
connection.register -> RegisterConnection
workspace.start      -> StartWorkspace
agent.message        -> RouteAgentMessage
```

它不查询 Workspace 或 Agent，不创建 ECS Entity，不决定 manager，不执行消息意图，也不检查资源权限。
WorkspacePlugin 负责逻辑 Workspace/Agent 路由，AgentPlugin 负责将 `AgentReference::Id` 解析为 Entity。

后端通过 `WebSocketMessageSend` 请求发送 `ServerEvent`。ApiPlugin 负责序列化和根据
`Broadcast`、`Type`、`Name` 筛选连接；业务 DTO 构造由 API 应用层负责。
