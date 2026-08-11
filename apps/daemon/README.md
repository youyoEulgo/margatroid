# Margatroid Daemon

daemon 是 Margatroid 的产品组合根，只负责读取启动配置、安装 Plugin 并运行 App。DtoPlugin 处理
WebSocket API 收发、DTO 与领域命令转换、完整状态及日志的出站投影，ConnectionPlugin 管理客户端
连接类型和名称。

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
├── config.toml
└── models.toml
```

`models.toml` 必须存在且符合 InferencePlugin 的模型路由格式；daemon 启动时会加载并校验它。
`config.toml` 保存全局 WebSocket 出站目标，暂定格式：

```toml
[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]
member_messages = ["type:webui"]
streaming_member_messages = ["type:webui"]
```

目标支持 `broadcast`、`type:<连接类型>` 和 `name:<连接名称>`。Workspace 启动、停止、失败或异常结果，以及成员
失败或异常，都归入 `logs`；完整成员消息和流式成员消息分别使用后两个字段。模型路由文件不再保存
WebSocket target。
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
省略 Agent 时由 WorkspacePlugin 查询 Workspace 的 manager。DtoPlugin 把请求转换成领域命令，
消息上下文、工具调用和推理由对应 Plugin 继续处理。

AgentPlugin 在消息处理或可见工具准备失败时发送 `agent.failure(kind=Agent)`；InferencePlugin 的请求
失败使用 `kind=Inference`。两者都会进入前端 Activity，不会伪造成对话历史。

所有出站消息的 target 都来自 `config.toml`。`state.sync` 是后端当前已就绪 Workspace、各 Agent
动态可见资源和可展示历史的完整快照，每次 Runtime tick 都会生成。历史直接来自各 Agent SQLite 的 `history_messages`；
`realtime_messages` 只用于恢复模型上下文，不发送给客户端。Web UI 必须以 `state.sync` 为业务状态的
唯一权威来源，不自行持久化、乐观追加或从实时事件拼接对话。
当前 CLI 仍只提交 Workspace 并打印日志，不负责 Agent 消息输入输出。
