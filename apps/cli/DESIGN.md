# MargatroidCli

## 类型

私有：
~~~text
Command：CLI命令，私有枚举--保存当前支持的命令参数
    Help--打印用法并正常退出
    WorkspaceUp {
        workspace_file: PathBuf--要编译的Workspace文件
        backend_url: String--后端WebSocket URL
    }
~~~

## 函数

公开：
~~~text
main()
    CLI入口：公开二进制入口，解析命令并将错误写入stderr后以非零状态退出
~~~

私有：
~~~text
run(command: Command) -> Result<(), Box<dyn Error + Send + Sync>>
    执行命令：私有异步函数，分派WorkspaceUp

run_workspace_up(workspace_file: PathBuf, backend_url: String) -> Result<(), Box<dyn Error + Send + Sync>>
    启动Workspace：私有异步函数，编译文件、连接WebSocket、注册cli连接、发送启动请求、处理关闭信号并打印日志事件
    行为：不读取stdin，不构造或发送LLM消息；ServerMessage::Log写入stdout；匹配启动请求ID的WorkspaceStartFailed立即返回错误

workspace_start_error(text: &str, request_id: &str) -> Option<String>
    解析启动失败：私有函数，只把匹配当前workspace.start请求ID的WorkspaceStartFailed转换为错误文本

print_backend_message(text: &str)
    打印日志：私有函数，反序列化ServerMessage并使用RFC 3339时间、等级、target、事件和结构化字段格式化LogRecordDto；未知事件直接忽略
    行为：stdout连接终端时启用ANSI等级色，并弱化时间和target；重定向时输出纯文本

print_cli_event(level: &str, message: &str)
    打印CLI事件：私有函数，使用当前RFC 3339时间、等级和margatroid_cli target写入stderr
    行为：stderr连接终端时使用与后端日志相同的ANSI样式

format_log_line(timestamp_millis: u64, level: &str, target: &str, message: &str, fields: &str, ansi: bool) -> String
    格式化日志行：私有函数，按等级选择ANSI颜色并将时间和target设为灰色弱化；ansi为false时不产生控制符

format_timestamp(timestamp_millis: u64) -> String
    格式化时间：私有函数，将Unix毫秒时间转换为RFC 3339 UTC文本

wait_for_stop_ack(socket, request_id: &str) -> Result<(), Box<dyn Error + Send + Sync>>
    等待停止回执：私有异步函数，等待同一请求ID的workspace.stopped
    行为：处理日志、心跳和停止失败；超时、断线或再次收到关闭信号时返回错误

wait_for_shutdown_signal() -> Result<(), std::io::Error>
    等待关闭信号：私有异步函数，Unix监听Ctrl+C和SIGTERM，其他平台监听Ctrl+C

parse_args<I>(arguments: I) -> Result<Command, String>
    解析参数：私有函数，接受workspace up、可选文件路径和--backend URL
    行为：文件省略时使用margatroid-workspace.yaml，backend省略时使用ws://127.0.0.1:3939/ws

request_id() -> String
    请求ID：私有函数，由进程ID和当前时间生成本次workspace.start的非空ID

usage() -> &'static str
    使用说明：私有函数，返回命令行用法文本
~~~

## 逻辑

~~~text
main
    -> parse_args(std::env::args().skip(1))
    -> run(command)
    -> 错误写stderr并返回非零状态

run_workspace_up
    -> compose::compile(workspace_file)
    -> request_id()
    -> ClientMessage::register_connection("cli")
    -> ClientMessage::start_workspace(id, &definition)
    -> 分别序列化两个ClientMessage
    -> tokio_tungstenite连接backend_url
    -> 依次发送connection.register和workspace.start的Message::Text
    -> 循环接收WebSocket消息和关闭信号
       -> 首次关闭信号：发送workspace.stop
       -> 等待同一请求ID的workspace.stopped
       -> 收到回执后关闭WebSocket并正常退出
       -> 再次信号、超时、停止失败或后端提前断开：关闭并报错退出
       -> Text：解析ServerMessage::Log并打印
       -> Text或UTF-8 Binary中的同ID workspace.start_failed：立即报错并以非零状态退出
       -> UTF-8 Binary：按文本解析ServerMessage::Log
       -> 非UTF-8 Binary：打印长度提示stderr
       -> Ping：发送Pong
       -> Close：打印关闭信息并结束
       -> 网络错误：返回错误

边界：
    CLI只负责Compose编译、Workspace启动请求和日志显示
    CLI不启动daemon、不创建ECS、不读取AgentImage或资源正文
    CLI不读取stdin，不处理UserMessage、AssistantMessage、ToolCall或LLM流
    WebSocket传输只承载margatroid_protocol定义的请求和后端事件
    正常运行期间收到关闭信号时，CLI退出前必须先完成workspace.stop业务回执；启动失败已由后端清理Workspace，直接报错退出，不发送workspace.stop
~~~

## 持有关系

~~~text
CLI进程
├── Command
├── WorkspaceDefinition（编译期间和请求构造期间持有）
├── ClientMessage（序列化前持有）
└── WebSocket连接
    └── 接收后端文本/二进制消息
~~~
