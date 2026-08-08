# Margatroid Daemon

daemon 是 Margatroid 的产品组合根，负责安装运行时和业务 Plugin。ApiPlugin 处理 WebSocket API
收发，ConnectionPlugin 管理客户端连接类型和名称。

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
├── skills/
├── workflows/
└── models.toml
```

`models.toml` 必须存在且符合 InferencePlugin 的模型路由格式；daemon 启动时会加载并校验它。
AgentImage 由 `agent-images/` 提供，Workspace 文件仍由 CLI 编译，daemon 不读取 YAML。
daemon 把 `skills/` 和 `workflows/` 分别交给 SkillPlugin 与 WorkflowPlugin。SkillPlugin 当前只读取
`SKILL.md`；WorkflowPlugin 当前只提供不执行正文的占位工具。

客户端连接后应先注册连接类型，再发送业务请求：

```text
connection.register--声明客户端类型，当前Web UI使用webui，CLI使用cli
workspace.start--提交CLI编译出的WorkspaceDefinition
agent.message--向已启动Workspace中的一个Agent发送用户消息
```

`agent.message` 使用 Workspace 名称和项目根目录定位实例。请求带 Agent 名称时投递给该成员；
省略 Agent 时查询 Workspace 的 manager。daemon 只把请求转换成内部 `AgentMessage`，消息上下文、
工具调用和推理由对应 Plugin 继续处理。

AgentPlugin 在消息处理或可见工具准备失败时发送 `agent.failure(kind=Agent)`；InferencePlugin 的请求
失败使用 `kind=Inference`。两者都会进入前端 Activity，不会伪造成对话历史。

结构化日志、`workspace.started`、`agent.message` 和 `agent.failure` 当前广播给全部连接。
`state.sync` 只发送给类型为 `webui` 的连接；它是后端当前已就绪 Workspace 和各 Agent 可展示历史的
完整快照，每次 Runtime tick 都会生成。历史直接来自各 Agent SQLite 的 `history_messages`；
`realtime_messages` 只用于恢复模型上下文，不发送给客户端。Web UI 必须以 `state.sync` 为业务状态的
唯一权威来源，不自行持久化、乐观追加或从实时事件拼接对话。
当前 CLI 仍只提交 Workspace 并打印日志，不负责 Agent 消息输入输出。
