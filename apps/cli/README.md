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
~~~

CLI 不读取 stdin，不发送用户消息、AgentMessage 或 LLM 请求；只处理 type 为 log 的消息并打印结构化
日志，其他后端事件忽略。非 UTF-8 二进制消息只打印长度提示。后端关闭连接或用户终止进程后命令结束。

后端必须提供对应的 WebSocket 路由，并能处理 margatroid_protocol::ClientMessage 的 JSON 形状。
