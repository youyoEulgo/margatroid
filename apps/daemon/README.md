# Margatroid Daemon

daemon 是 Margatroid 的产品组合根，只负责读取启动配置、安装 Plugin 并运行 App。DtoPlugin 处理
WebSocket API 收发、DTO 与领域命令转换、完整状态及日志的出站投影，ClientPlugin 管理客户端
连接类型和名称，并把注册成功的客户端实体化为 Client Entity。

启动：

```text
cargo run -p margatroid_daemon
```

daemon不接受启动参数，固定使用 `~/.margatroid` 主目录。监听地址和其他运行配置全部来自
`~/.margatroid/config.toml`。

数据目录至少包含：

```text
~/.margatroid/
├── agent-images/
├── skills/
├── tools/
├── workflows/
├── config.toml
└── models.toml
```

`models.toml` 必须存在且符合 InferencePlugin 的模型路由格式；daemon 启动时会加载并校验它。
`config.toml` 保存全局 WebSocket 出站目标，暂定格式：

```toml
[server]
bind = "127.0.0.1:3939"
allow = "localhost"

[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]
member_messages = ["type:webui"]
streaming_member_messages = ["type:webui"]
```

`server.bind` 是ServerPlugin的监听地址。`server.allow` 是入口策略：默认 `"localhost"` 只接受回环对端，
也可以写 `"all"`（需同时放宽 `bind`）或 IP/CIDR 数组；带 `Origin` 的请求还要求其 authority 与 `Host` 一致，
不通过的升级请求在握手阶段返回 403、socket 不建立。`allow` 与 `bind` 不一致时 daemon 在启动时报错。目标支持 `broadcast`、`type:<连接类型>` 和
`name:<连接名称>`。Workspace 启动、停止、失败或异常结果，以及成员
失败或异常，都归入 `logs`；完整成员消息和流式成员消息分别使用后两个字段。模型路由文件不再保存
WebSocket target。
AgentImage 由 `agent-images/` 提供，Workspace 文件仍由 CLI 编译，daemon 不读取 YAML。
daemon 把主目录交给 ToolPlugin；ToolPlugin 的 handler 内置 skill/hook/lua/shell 执行器。
LLM只看到由这些执行器注册的 `skill:*`、`tool:*` 和 `shell:*` 资源，不会看到 `tool:builtin/*`。

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
SQLite 的 `state` 表只保存 driver 声明的 state（含实时上下文快照），不发送给客户端。Web UI 必须以 `state.sync` 为业务状态的
唯一权威来源，不自行持久化、乐观追加或从实时事件拼接对话。
当前 CLI 仍只提交 Workspace 并打印日志，不负责 Agent 消息输入输出。
