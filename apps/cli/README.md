# Margatroid CLI

当前 CLI 只实现 Workspace 启动请求和后端日志显示，不实现 LLM 消息输入或输出。

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
-> 首次 Ctrl+C/SIGTERM：WebSocket 发送 workspace.stop
-> 等待匹配 request id 的 workspace.stopped
-> 收到回执后关闭 WebSocket 并退出
~~~

CLI 不读取 stdin，不发送用户消息、AgentMessage 或 LLM 请求；只处理 type 为 log 的消息并打印结构化
日志，其他后端事件忽略。非 UTF-8 二进制消息只打印长度提示。关闭信号不会直接终止进程，而是先请求后端停止
workspace；停止失败、超时、重复信号或后端提前断开时命令报错退出。

CLI 生命周期事件与后端日志统一显示 RFC 3339 UTC 时间、等级和事件目标，例如：

```text
2026-08-09T13:19:03.032Z INFO  dto_plugin::outbound: workspace started (request_id=...)
```

连接终端时，时间和事件目标使用灰色弱化，`INFO` 为绿色，`WARN` 为黄色，`ERROR` 为红色，
`DEBUG` 为蓝色，`TRACE` 为紫色。重定向到文件或管道时自动输出不含 ANSI 控制符的纯文本。

后端必须提供对应的 WebSocket 路由，并能处理 margatroid_protocol::ClientMessage 的 JSON 形状。
