# Margatroid Protocol

`margatroid_protocol` 保存客户端与 daemon 之间共享的跨进程 DTO。它不依赖 ECS、WorkspacePlugin
或网络实现；WebSocket、HTTP 等传输层只负责传递其 JSON 表示。

客户端建立 WebSocket 连接后首先声明客户端类型：

```json
{
  "type": "connection.register",
  "client_type": "webui"
}
```

当前 Web UI 使用 `webui`，CLI 使用 `cli`。协议只传递该字符串；类型校验、连接类型写入和唯一名称
生成由 ConnectionPlugin 负责。

启动 Workspace：

```rust
use margatroid_protocol::ClientRequest;

let request = ClientRequest::start_workspace("request-1", &definition);
let json = serde_json::to_string(&request)?;
```

启动完成后，daemon 发送 `workspace.started`，返回 Workspace 逻辑身份、manager 和所有可选 Agent：

```json
{
  "type": "workspace.started",
  "id": "request-1",
  "workspace": {
    "name": "demo",
    "project_root": "/project/demo",
    "manager": "coder",
    "agents": ["coder", "reviewer"]
  }
}
```

客户端连接后还会收到 `state.sync`。它包含 daemon 当前全部已就绪 Workspace，以及每个 Agent 从
`history_messages` 读取的完整可展示历史。客户端应以该快照整体替换本地视图，不自行持久化、乐观追加
或从 `agent.message` 拼接对话：

```json
{
  "type": "state.sync",
  "state": {
    "workspaces": [
      {
        "name": "demo",
        "project_root": "/project/demo",
        "manager": "coder",
        "agents": ["coder", "reviewer"]
      }
    ],
    "histories": [
      {
        "workspace": {
          "name": "demo",
          "project_root": "/project/demo"
        },
        "agent": "coder",
        "messages": [
          {
            "sequence": 1,
            "turn_id": "message-1",
            "message": { "User": { "content": "Hello." } },
            "resources": [
              { "provider": "skill", "name": "local/project-context" }
            ],
            "created_at_ms": 1720000000000
          }
        ]
      }
    ]
  }
}
```

`histories` 不包含 System、Tool 响应或资源正文。`resources` 只标记该条用户消息实际使用了哪些资源。
用于恢复模型上下文的 `realtime_messages` 不属于客户端协议。

客户端向指定成员发送用户消息：

```json
{
  "type": "agent.message",
  "id": "message-1",
  "workspace": {
    "name": "demo",
    "project_root": "/project/demo"
  },
  "agent": "reviewer",
  "content": "Review this change."
}
```

`agent` 可以为 `null`；daemon 此时查询 Workspace 的 manager 并把它作为目标。显式名称不存在、
Workspace 尚未就绪、ID 为空或正文为空时，请求不会创建内部消息，错误暂时通过日志报告。

Agent 的对话事件使用同一个 `agent.message` 类型返回，并带上已经解析出的 Agent 名称。当前
`margatroid_types::Message` 使用 serde 的枚举形状，例如用户消息为：

```json
{
  "type": "agent.message",
  "message": {
    "id": "message-1",
    "workspace": {
      "name": "demo",
      "project_root": "/project/demo"
    },
    "agent": "reviewer",
    "message": {
      "User": {
        "content": "Review this change."
      }
    }
  }
}
```

无法表示成消息的 Agent 轮次失败使用 `agent.failure` 返回。`kind` 对应领域失败来源：`Agent` 表示
消息处理或可见工具准备失败，`Inference` 表示模型请求准备或执行失败。

后端日志继续使用 `ServerEvent::Log` 发送；现有 CLI 只处理这种事件，不承担 Agent 消息输入输出。

```json
{
  "type": "log",
  "record": {
    "timestamp_millis": 1720000000000,
    "level": "INFO",
    "target": "workspace_plugin",
    "message": "workspace started",
    "fields": [],
    "spans": []
  }
}
```

JSON 形状：

```json
{
  "type": "workspace.start",
  "id": "request-1",
  "definition": {
    "name": "demo",
    "project_root": "/project/demo",
    "manager": "coder",
    "agents": [
      {
        "name": "coder",
        "image": "local/coder:latest",
        "resources": [
          { "provider": "skill", "name": "local/project-context" }
        ],
        "disable_resources": [],
        "memory_path": null
      }
    ]
  }
}
```

`WorkspaceDefinitionDto` 将镜像、资源和路径转换为跨进程稳定的字符串形式；后端收到 DTO 后可以
调用 `into_definition` 恢复 `margatroid_types` 中的领域值，随后仍由 WorkspacePlugin 做运行时
复核。协议只携带逻辑 Workspace/Agent 身份，不暴露 ECS `Entity`，也不决定 manager、消息意图或
模型请求。
