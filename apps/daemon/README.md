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
├── agent-drivers/
├── tools/
├── workflows/
├── config.toml
└── models.toml
```

`models.toml` 必须存在且符合 InferencePlugin 的模型路由格式；daemon 启动时会加载并校验它。

推理分层已实现：provider必填，取openai/anthropic/deepseek/openrouter；api_type为chat_completions/messages/responses。显式七组合为OpenAI chat+responses、Anthropic messages、DeepSeek三种、OpenRouter chat（2+1+3+1）。逻辑ID、上游model、provider和api_type各自独立，未知值/组合/选项在加载或重载时失败；失败保留旧完整路由及安全catalog。没有工厂注册表或URL猜测。base_url是API前缀，只追加/chat/completions、/messages或/responses；DeepSeek兼容Messages如需/anthropic/v1/messages，前缀由用户显式设为/anthropic/v1。主目录两条路由已迁移api_type=chat_completions并保留各自provider、凭据；config.toml不新增重复模型配置。Responses每次提交完整本地上下文，store=false、include=["reasoning.encrypted_content"]，不使用conversation/previous_response_id。Assistant可选replay保存Messages签名/遮蔽thinking和Responses reasoning id/加密内容/summary；不兼容格式或无法表达的参数在HTTP前拒绝。User 可含 images 原始 base64 图片数组，三种 API 分别转换为 image_url、image.source 或 input_image；纯文本请求保持原格式，上游模型是否支持视觉仍由模型决定。
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
AgentDriver 由 `agent-drivers/` 提供，Workspace 文件仍由 CLI 编译，daemon 不读取 YAML。
仓库 `resources/` 包含 coder 和 reviewer 两个 Driver 及其依赖；安装时保持目录结构复制到主目录。
reviewer 使用独立 Lua 实例、会话和历史数据库，默认启用 read-only 沙箱，可见工具为 code-review、project-context 和 list-directory。
双成员 Workspace 可用 coder 作为 manager，同时声明 `local/coder:latest` 和 `local/reviewer:latest`；离线集成测试覆盖两者同时就绪和独立运行。旧 `agent-images/` 目录不参与 Driver 加载。
内置 coder 可见 read_image 工具：Lua 在当前沙箱内读取 PNG/JPEG/GIF/WebP 文件（相对路径基于 project_root），返回含独立 images 的工具结果。Messages 与 Responses 使用原生含图片工具结果，Chat 在整批工具结果齐全之后附加 User 图片消息，仅转换 HTTP 上下文，不改写本地历史身份。已有持久 session.visible 不被启动脚本自动覆盖，升级旧会话需显式导入并加入该工具或使用新会话。
User 消息可携带独立 images 数组，图片为 media_type 和原始 base64 data，不混入 content；MCL、Lua 会话、SQLite 历史和恢复均须保留图片，history.content 仍只保存文本。默认 Driver 无须按 API 分叉；离线回归覆盖带图片用户消息经过真实 Driver 后进入推理、历史、停止并重启恢复。API 图片转换不等于任意已配置模型都具备视觉能力。
daemon 把主目录交给 ToolResourcePlugin 加载工具资源，ToolPlugin 只消费已选定的 Tool 快照，
通过独立 Lua runner 执行。LLM 的 TOOL 数组只包含 `tool:*` 资源；prompt 和 sandbox 使用各自 typed array。
命令统一由 Lua 工具的 `margatroid().shell` 经 runner 执行，shell 不是单独的可调用资源。

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
Daemon 显式将 InferencePlugin 安排在 UPDATE，确保 PRE_UPDATE 的用户中止先于本帧推理 admission；取消已运行推理时本地断开 HTTP/SSE，累计思考和正文以 cancelled Assistant 入历史但不入上下文，未完成工具调用不执行。该本地断连不保证供应商停止计算或计费。

Workspace 成员启动隔离：初始请求全部收口即 workspace.started，成员启动或驱动加载失败保存在 Workspace.member_failures 并投影到快照及逐成员 logs，不关闭健康工作区。全部失败也可正常停止并以同 ID 重启；真正定义/创建失败不占用新地址，重复地址保留原实例并返回含状态和成员计数的诊断。
