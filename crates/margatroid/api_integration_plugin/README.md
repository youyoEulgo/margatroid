# ApiIntegrationPlugin

`ApiIntegrationPlugin` 是领域运行时到 `ApiPlugin` 的应用层投影插件。它把 Workspace、Agent、Memory、
Server 和日志状态投影为 `WebSocketMessageSend`。

边界如下：

- `ApiPlugin` 处理 WebSocket 文本、协议 JSON、DTO 转换和领域命令发送；
- `ApiIntegrationPlugin` 只负责领域事件到客户端 DTO 的投影；
- 领域 Plugin 不依赖 WebSocket 或客户端连接；
- daemon 只负责配置、Plugin 装配和启动。

默认每次 `RuntimePlugin::UPDATE` 生成一次后端完整状态，发送给类型为 `webui` 的连接。可通过
`with_frontend_type` 修改目标类型。
