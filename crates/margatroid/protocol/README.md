# Margatroid Protocol

`margatroid_protocol` 定义客户端与 daemon 之间的 JSON 协议 DTO。所有客户端请求统一使用三个
顶层字段：`type`、`id` 和 `message`。

DTO 转换统一使用 `FromDomain` 和 `IntoDomain`。领域类型根据这些实现自动获得 `IntoDto` 和
`FromDto`。转换上下文由具体 DTO 决定：纯静态转换使用 `()`，需要查询 ECS 身份时使用 `&World`，
请求转换可以使用请求 ID 或连接 ID。

连接注册请求：

```json
{
  "type": "connection.register",
  "id": "register-1",
  "message": {
    "client_type": "webui"
  }
}
```

Workspace 启动请求：

```json
{
  "type": "workspace.start",
  "id": "request-1",
  "message": {
    "definition": {
      "name": "demo",
      "project_root": "/project/demo",
      "manager": "coder",
      "agents": []
    }
  }
}
```

Workspace 停止请求和业务回执：

```json
{
  "type": "workspace.stop",
  "id": "stop-1",
  "message": {
    "workspace": {
      "name": "demo",
      "project_root": "/project/demo"
    }
  }
}
```

后端完成停止后发送 `workspace.stopped`（复用请求 ID）。停止失败发送
`workspace.stop_failed`。客户端应等待成功回执后再关闭 WebSocket。

Agent 消息请求：

```json
{
  "type": "agent.message",
  "id": "message-1",
  "message": {
    "workspace": {
      "name": "demo",
      "project_root": "/project/demo"
    },
    "agent": "coder",
    "message": {
      "content": "Review this change."
    }
  }
}
```

DtoPlugin 反序列化 DTO 后直接调用 `into_domain` 发送领域事件，不再发送只复制字段的
API 中间事件。Agent 逻辑名称由 WorkspacePlugin 在发送 `AgentMessage` 前解析为 ECS Entity；出站
DTO 再通过 `World` 将 Entity 投影为稳定 Agent ID，协议不会暴露 Entity。

客户端用户输入使用 `UserMessageDto`，不能构造 System、Assistant 或 Tool 消息。后端可展示消息使用
`MessageDto`，只包含 User 和 Assistant；领域 `Message` 与 `ToolCall` 不直接进入协议字段。

后端状态通过 `state.sync` 发送完整快照。前端以该快照替换业务状态，不自行持久化或从增量消息拼接
历史。`histories` 只包含可展示的 User/Assistant 消息和资源引用，不包含 Tool、System 或资源正文。
