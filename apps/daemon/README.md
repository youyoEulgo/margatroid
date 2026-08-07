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

客户端连接后可以发送两类请求：

```text
workspace.start--提交CLI编译出的WorkspaceDefinition
agent.message--向已启动Workspace中的一个Agent发送用户消息
```

`agent.message` 使用 Workspace 名称和项目根目录定位实例。请求带 Agent 名称时投递给该成员；
省略 Agent 时查询 Workspace 的 manager。daemon 只把请求转换成内部 `AgentMessage`，消息上下文、
工具调用和推理由对应 Plugin 继续处理。

daemon 会向 WebSocket 广播结构化日志、`workspace.started`、`agent.message` 和 `agent.failure`。
当前 CLI 仍只提交 Workspace 并打印日志，不负责 Agent 消息输入输出。
