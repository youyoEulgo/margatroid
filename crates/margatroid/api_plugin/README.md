# ApiPlugin

`ApiPlugin` 是 Margatroid 的 WebSocket API 路由层。它把 ServerPlugin 产生的
`WebSocketMessageReceived` 解析为 `ClientRequest`，再按请求类型发送内部事件；后端输出统一通过
`WebSocketMessageSend` 进入插件，由插件序列化 `ServerEvent` 并发送。

当前输入事件：

```text
connection.register -> ConnectionRegisterRequested
workspace.start      -> WorkspaceStartRequested
agent.message        -> AgentMessageRequested
```

发送目标支持全部连接、指定连接类型和指定连接名称：

```text
WebSocketMessageTarget::Broadcast
WebSocketMessageTarget::Type("webui")
WebSocketMessageTarget::Name("webui-12")
```

ApiPlugin 不校验业务字段、不查询 Workspace 或 Agent，也不生成连接名称。连接元数据由
ConnectionPlugin 管理。
