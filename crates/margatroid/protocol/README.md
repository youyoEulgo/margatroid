# Margatroid Protocol

`margatroid_protocol` 定义客户端与 daemon 之间的 JSON 协议 DTO。所有客户端请求统一使用三个
顶层字段：`type`、`id` 和 `message`。

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
      "User": {
        "content": "Review this change."
      }
    }
  }
}
```

ApiPlugin 反序列化 DTO 后直接调用 `into_domain` 发送领域事件，不再发送只复制字段的
API 中间事件。Agent 逻辑名称最终由 WorkspacePlugin
映射到稳定 Agent ID，再由 AgentPlugin 解析为 ECS Entity；协议不会暴露 Entity。

后端状态通过 `state.sync` 发送完整快照。前端以该快照替换业务状态，不自行持久化或从增量消息拼接
历史。`histories` 只包含可展示的 User/Assistant 消息和资源引用，不包含 Tool、System 或资源正文。
