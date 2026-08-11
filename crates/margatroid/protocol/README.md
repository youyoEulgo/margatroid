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

启动成功时后端发送复用请求ID的`workspace.started`；启动失败时发送
`workspace.start_failed { id, error }`。两者都是`workspace.start`的终止回执，客户端不应通过
日志正文推断启动结果。

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
    },
    "tool_calls": []
  }
}
```

DtoPlugin 反序列化 DTO 后直接调用 `into_domain` 发送领域事件，不再发送只复制字段的
API 中间事件。Agent 逻辑名称由 WorkspacePlugin 在发送 `AgentMessage` 前解析为 ECS Entity；出站
DTO 再通过 `World` 将 Entity 投影为稳定 Agent ID，协议不会暴露 Entity。

客户端用户输入使用 `UserMessageDto`，不能构造 System、Assistant 或 Tool 消息。后端可展示消息使用
`MessageDto`，包含 User、Assistant 和历史Tool；领域 `Message` 与 `ToolCall` 不直接进入协议字段。

后端状态通过 `state.sync` 发送完整快照。前端以该快照替换业务状态，不自行持久化或从增量消息拼接
历史。`histories` 包含可展示的 User、Assistant 和 Tool消息，不包含 System 或Skill正文。`agents`
包含每个运行中Agent的`visible_resources`，该字段直接来自当前`AgentDynamicVisibility`而不是镜像
默认可见性；客户端可以按`provider`筛选资源，例如只展示可手动调用的Skill。

## 流式 Agent 响应

模型响应的文本分片通过 `agent.message.delta` 转发。分片只用于当前回复的实时渲染，不写入历史表，
并与最终 `agent.message` 使用相同的轮次 ID。前端为每个 Agent 保存当前轮次的消息累积器，收到
分片后直接追加并渲染。

推理完成后，后端仍发送完整的 `agent.message`，用于上下文、记忆和最终校正。前端必须按轮次 ID
将它与当前累积器比较，相同则丢弃，不同则用完整消息替换当前内容。随后发送的 `state.sync` 必须
确认该轮次已经进入后端历史；前端只有确认后才清空累积器并从完整历史重新渲染。

完整 `agent.message` 同时是该轮流式响应的完成标记。后端必须保证同一发送目标下的所有分片先
进入 WebSocket 发送顺序，再发送完整消息。发送器句柄可以复制，不要求分片和完整消息使用同一个
句柄；只要两者的 `target` 相同即可。前端只屏蔽已经完成的同一轮次分片，不影响后续新轮次。流式
状态按 `agent` 和 `id` 管理，避免不同轮次的内容互相覆盖。按连接类型筛选的动态目标应在一轮响应
开始时固定连接集合，或明确规定中途加入的连接从下一轮开始接收。
