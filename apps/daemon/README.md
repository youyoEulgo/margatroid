# Margatroid Daemon

daemon 是 Margatroid 的产品组合根，负责安装运行时和业务 Plugin，监听 CLI 的 WebSocket
请求，并把结构化日志转发给连接中的客户端。

启动：

```text
cargo run -p margatroid_daemon -- --data-root ~/.margatroid
```

默认监听 `127.0.0.1:3939`，CLI 连接 `ws://127.0.0.1:3939/ws`。可用参数：

```text
--bind HOST:PORT          监听地址，默认 127.0.0.1:3939
--data-root DIRECTORY     数据目录，默认 ~/.margatroid
```

数据目录至少包含：

```text
~/.margatroid/
├── agent-images/
└── models.toml
```

`models.toml` 必须存在且符合 InferencePlugin 的模型路由格式；daemon 启动时会加载并校验它。
AgentImage 由 `agent-images/` 提供，Workspace 文件仍由 CLI 编译，daemon 不读取 YAML。

CLI 连接后发送 `workspace.start`，daemon 将请求恢复为领域定义并交给 WorkspacePlugin。后端
只向 WebSocket 转发日志，不接收用户消息，也不处理 LLM 输入输出。
