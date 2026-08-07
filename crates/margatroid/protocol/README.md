# Margatroid Protocol

`margatroid_protocol` 保存 CLI 与 daemon 之间共享的跨进程 DTO。它不依赖 ECS、WorkspacePlugin 或
网络实现；WebSocket、HTTP 等传输层只负责传递其 JSON 表示。

当前只定义 Workspace 启动请求：

```rust
use margatroid_protocol::ClientRequest;

let request = ClientRequest::start_workspace("request-1", &definition);
let json = serde_json::to_string(&request)?;
```

后端日志使用 `ServerEvent::Log` 发送，CLI 只处理这种事件。其他后端事件可以继续扩展，但不属于
CLI 的 LLM 消息输入输出通道。

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
复核。协议 crate 不发送或解析 LLM 消息。
