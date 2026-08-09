# ApiIntegrationPlugin

`ApiIntegrationPlugin` 负责触发完整前端状态和日志集成。具体 DTO 转换由 Protocol 的 `FromDomain`
实现完成，它只把转换结果包装为 `WebSocketMessageSend`。

边界如下：

- `DtoPlugin` 处理 WebSocket 文本、协议 JSON、DTO 转换和领域命令发送；
- `DtoPlugin` 收集允许发送给外部的即时领域事件并构造客户端 DTO；
- `ApiIntegrationPlugin` 只负责完整状态快照和日志转发；
- 领域 Plugin 不依赖 WebSocket 或客户端连接；
- daemon 只负责配置、Plugin 装配和启动。

默认每次 `RuntimePlugin::UPDATE` 生成一次后端完整状态，发送给类型为 `webui` 的连接。可通过
`with_frontend_type` 修改目标类型。
