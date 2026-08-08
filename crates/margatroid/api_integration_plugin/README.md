# ApiIntegrationPlugin

`ApiIntegrationPlugin` 是领域运行时与 `ApiPlugin` 之间的应用层适配器。它把客户端的
Workspace/Agent 请求转换为领域事件，并把 Workspace、Agent、Memory、Server 和日志状态投影为
`WebSocketMessageSend`。

边界如下：

- `ApiPlugin` 只处理 WebSocket 文本、协议 JSON 和连接目标；
- `ApiIntegrationPlugin` 解析逻辑 Workspace/Agent 身份并构造客户端 DTO；
- 领域 Plugin 不依赖 WebSocket、客户端连接或协议 DTO；
- daemon 只负责配置、Plugin 装配和启动。

默认每次 `RuntimePlugin::UPDATE` 生成一次后端完整状态，发送给类型为 `webui` 的连接。可通过
`with_frontend_type` 修改目标类型。
