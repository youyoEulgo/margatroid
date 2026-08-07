# MargatroidDaemon

## 类型

私有：
~~~text
DaemonConfig：daemon启动配置，私有结构体
    bind: SocketAddr--ServerPlugin监听地址
    data_root: PathBuf--AgentImage和模型配置根目录

DaemonStart：daemon首帧唤醒事件，私有单元结构体--触发事件驱动Runtime执行Startup
~~~

## 函数

公开：
~~~text
main()
    daemon入口：公开二进制入口，解析参数、构造App并运行事件驱动Runtime
~~~

私有：
~~~text
run(config: DaemonConfig) -> Result<(), Error>
    启动daemon：创建数据目录，安装Plugin组合，注册/ws路由和系统，启动日志转发并运行App

install_log_forwarder(app: &App) -> Result<(), Error>
    安装日志转发：取得LogPlugin的TracingStream和ServerPlugin的WebSocketConnections，提交异步转发服务

forward_logs(stream: TracingStream, connections: WebSocketConnections)
    转发日志：订阅结构化Tracing记录，转换成ServerEvent::Log JSON并发送给所有未命名WebSocket连接

handle_websocket_messages(world: &mut World)
    处理客户端消息：只接受文本，解析ClientRequest，恢复WorkspaceDefinition并发送StartWorkspace
    非法协议、非文本消息和定义恢复错误只记录日志，不创建业务事件

report_runtime_events(world: &mut World)
    报告运行结果：读取ServerStarted、ServerFailed、ServerStopped和StartWorkspaceResult，使用tracing记录服务与Workspace状态

parse_args(arguments) -> Result<DaemonConfig, String>
    解析--bind和--data-root，缺省使用127.0.0.1:3939和~/.margatroid

default_data_root() -> PathBuf
    默认数据目录：从HOME构造~/.margatroid，HOME缺失时使用当前目录下.margatroid

absolute_path(path: &Path) -> Result<PathBuf, Error>
    规范化数据根：相对路径拼接当前目录，绝对路径原样返回
~~~

## 逻辑

~~~text
main
    -> parse_args
    -> run

run
    -> 创建data_root
    -> AgentImageLoaderPlugin::open(data_root/agent-images)
    -> WorkspacePlugin::open(data_root/agent-images)
    -> 安装RuntimePlugin
    -> 安装AsyncRuntimePlugin
    -> 安装LogPlugin(with_stream)
    -> 安装ServerPlugin
    -> 安装AgentImageLoaderPlugin
    -> 安装InferencePlugin(with_config_path(data_root/models.toml))
    -> 安装ToolPlugin、MemoryPlugin、AgentPlugin、WorkspacePlugin
    -> 注册/ws WebSocket事件路由
    -> 注册客户端消息和运行结果System
    -> 启动日志异步转发
    -> 发送DaemonStart唤醒事件，保证事件驱动Runtime执行首个Startup阶段
    -> AppRunExt::run

WebSocketMessageReceived
    -> handle_websocket_messages
    -> ClientRequest::WorkspaceStart
    -> WorkspaceDefinitionDto::into_definition
    -> StartWorkspace
    -> WorkspacePlugin异步加载AgentImage并创建Workspace/Agent

StartWorkspaceResult
    -> report_runtime_events
    -> tracing::info或tracing::error
    -> TracingStream
    -> ServerEvent::Log
    -> WebSocketConnections中的CLI
~~~

## 持有关系

~~~text
daemon App
├── RuntimePlugin
├── AsyncRuntimePlugin
├── LogPlugin
│   └── TracingStream
├── ServerPlugin
│   └── WebSocketConnections
├── AgentImageLoaderPlugin
├── InferencePlugin
├── ToolPlugin
├── MemoryPlugin
├── AgentPlugin
└── WorkspacePlugin
~~~

daemon只负责产品组合、网络入口和日志出口。Workspace编译、AgentImage读取、Workspace创建、
记忆持久化、工具执行和模型请求分别属于对应Plugin；daemon不解析YAML、不检查资源可见性、
不接收用户消息，也不实现LLM消息输入输出。
