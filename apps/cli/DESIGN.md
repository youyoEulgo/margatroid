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
    启动Workspace：私有异步函数，编译文件、连接WebSocket、注册cli连接、发送启动请求并打印日志事件
    行为：不读取stdin，不构造或发送LLM消息；只有ServerMessage::Log写入stdout

print_backend_message(text: &str)
    打印日志：私有函数，反序列化ServerMessage并格式化LogRecordDto；未知事件直接忽略

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
    -> 循环接收WebSocket消息
       -> Text：解析ServerMessage::Log并打印
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
