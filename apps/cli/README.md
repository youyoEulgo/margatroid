# Margatroid CLI

当前 CLI 只实现 Workspace 启动请求和后端日志显示，不实现 LLM 消息输入或输出。
命令行参数由 `clap` 解析，可在任意命令层级使用 `--help` 查看对应说明。

~~~text
margatroid workspace up [WORKSPACE_FILE] [--backend WS_URL]
~~~

WORKSPACE_FILE 省略时使用当前目录的 margatroid-workspace.yaml。--backend 省略时连接：

~~~text
ws://127.0.0.1:3939/ws
~~~

执行流程：

~~~text
读取 Workspace 文件
-> Compose::compile
-> margatroid_protocol::ClientMessage::WorkspaceStart
-> WebSocket 发送 connection.register(client_type=cli)
-> WebSocket 发送 workspace.start
-> 持续接收并打印后端消息
-> 收到匹配请求ID的 workspace.started：显示成功和成员失败详情，继续连接
-> 成功前收到匹配请求ID的 workspace.start_failed：报错并以非零状态退出
-> 首次 Ctrl+C/SIGTERM：启动回执未到时保留停止意图；启动成功后 WebSocket 发送 workspace.stop
-> 等待匹配 request id 的 workspace.stopped
-> 收到回执后关闭 WebSocket 并退出
~~~

CLI 不读取 stdin，不发送用户消息、AgentMessage 或 LLM 请求。打印 `log` 消息及匹配启动请求的
`workspace.started` 摘要；成功回执中的 `member_failures` 逐条显示 Agent、Driver 和具体原因，但不会使 CLI 退出。
即使全部初始成员失败，已建立的 Workspace 仍保持连接并可正常停止；manager 不可用不等于 Workspace 未建立。
只有成功回执之前匹配当前请求的 `workspace.start_failed`（Workspace 自身无法创建），或协议预检的 `request.failed`（请求被拒绝、尚未创建），才作为启动终止错误。
后端必须确保这种失败不留下本次创建的占位；已有同地址 Workspace 的冲突不覆盖旧实例，诊断包含旧实例状态和成员情况。
成员后续失败不改变 CLI 生命周期；成功回执后重复或延迟的启动失败回执不再终止会话。
非 UTF-8 二进制消息只打印长度提示。关闭信号不会直接终止进程，而是先请求后端停止
workspace；停止失败、超时、重复信号或后端提前断开时命令报错退出。

CLI 生命周期事件与后端日志统一显示 RFC 3339 UTC 时间、等级和事件目标，例如：

```text
2026-08-09T13:19:03.032Z INFO  dto_plugin::outbound: workspace started (request_id=...)
```

连接终端时，时间和事件目标使用灰色弱化，`INFO` 为绿色，`WARN` 为黄色，`ERROR` 为红色，
`DEBUG` 为蓝色，`TRACE` 为紫色。重定向到文件或管道时自动输出不含 ANSI 控制符的纯文本。

后端必须提供对应的 WebSocket 路由，并能处理 margatroid_protocol::ClientMessage 的 JSON 形状。
