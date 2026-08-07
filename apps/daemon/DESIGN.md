# MargatroidDaemon

## 类型

私有：
```text
DaemonConfig：daemon启动配置，私有结构体
    bind: SocketAddr--ServerPlugin监听地址
    data_root: PathBuf--AgentImage和模型配置根目录

DaemonStart：daemon首帧唤醒事件，私有单元结构体--触发事件驱动Runtime执行首个Startup阶段
    impl Event for DaemonStart
```

## 函数

公开：
```text
main()
    daemon入口：解析参数、构造Plugin组合并运行App
```

私有：
```text
run(config: DaemonConfig) -> Result<(), Error>
    启动daemon：创建数据目录，安装运行时和业务Plugin，注册/ws路由，启动日志转发并运行App

install_log_forwarder(app: &App) -> Result<(), Error>
    安装日志转发：取得LogPlugin的TracingStream和ServerPlugin的WebSocketConnections，提交异步服务

forward_logs(stream: TracingStream, connections: WebSocketConnections)
    转发日志：订阅结构化Tracing记录，转换为ServerEvent::Log并发送给未命名WebSocket连接

handle_websocket_messages(world: &mut World)
    处理客户端请求：读取文本WebSocket事件，解析ClientRequest并提交对应内部事件
    行为：
        workspace.start恢复WorkspaceDefinition后发送StartWorkspace
        agent.message校验ID和正文，按WorkspaceRefDto查找Workspace
        agent.message带Agent名称时查找该成员；省略名称时查找Workspace.manager
        查找成功后发送margatroid_types::AgentMessage { Message::User, UserWithoutToolCalls }
        非文本、非法JSON、定义恢复失败和路由失败只记录日志，不创建后续业务事件

report_runtime_events(world: &mut World)
    报告运行事件：记录Server生命周期与Workspace失败，并广播Workspace和Agent事件
    行为：
        StartWorkspaceResult成功时广播workspace.started，携带Workspace名称、项目根、manager和Agent列表
        margatroid_types::AgentMessage按Agent反查Workspace身份后广播agent.message
        AgentFailure按Agent反查Workspace身份后广播agent.failure
        ServerStarted、ServerFailed、ServerStopped只写tracing日志

route_agent_message(
    world: &World,
    id: String,
    workspace: WorkspaceRefDto,
    agent: Option<String>,
    content: String,
) -> Result<(), String>
    路由用户消息：完成输入校验、Workspace查找、显式Agent或manager查找和内部AgentMessage发送

workspace_info(world: &World, workspace: Entity) -> Option<WorkspaceInfoDto>
    构造Workspace信息：从WorkspaceConfiguration转换协议DTO

agent_route(world: &World, agent: Entity) -> Option<(WorkspaceRefDto, String)>
    反查Agent身份：通过WorkspacePlugin索引取得Workspace引用和Agent名称

broadcast_server_event(connections: &WebSocketConnections, event: &ServerEvent)
    广播协议事件：序列化JSON并用非阻塞try_send发送给所有未命名连接

parse_args(arguments) -> Result<DaemonConfig, String>
    解析--bind和--data-root，缺省使用127.0.0.1:3939和~/.margatroid

parse_bind(value: String) -> Result<SocketAddr, String>
    解析监听地址：将文本转换为SocketAddr并返回有界错误描述

default_data_root() -> PathBuf
    默认数据根：从HOME构造~/.margatroid，HOME缺失时使用当前目录下.margatroid

absolute_path(path: &Path) -> Result<PathBuf, Error>
    规范化数据根：相对路径拼接当前目录，绝对路径原样返回

usage() -> &'static str
    返回命令行用法文本
```

## 逻辑

```text
main
    -> parse_args
    -> run

run
    -> 创建data_root
    -> AgentImageLoaderPlugin::open(data_root/agent-images)
    -> WorkspacePlugin::open(data_root/agent-images)
    -> 安装RuntimePlugin、AsyncRuntimePlugin、LogPlugin和ServerPlugin
    -> 安装AgentImageLoaderPlugin、InferencePlugin、ToolPlugin、MemoryPlugin、AgentPlugin、WorkspacePlugin
    -> 注册/ws WebSocket事件路由
    -> 注册handle_websocket_messages和report_runtime_events
    -> 安装异步日志转发
    -> 发送DaemonStart
    -> AppRunExt::run

客户端Workspace启动：
    WebSocketMessageReceived
        -> handle_websocket_messages
        -> ClientRequest::WorkspaceStart
        -> WorkspaceDefinitionDto::into_definition
        -> StartWorkspace
        -> WorkspacePlugin创建Workspace和Agent
        -> StartWorkspaceResult
        -> report_runtime_events
        -> ServerEvent::WorkspaceStarted

客户端Agent消息：
    WebSocketMessageReceived
        -> handle_websocket_messages
        -> ClientRequest::AgentMessage
        -> WorldWorkspaceExt::workspace
        -> WorldWorkspaceExt::workspace_agent或workspace_manager
        -> margatroid_types::AgentMessage
        -> AgentPlugin和后续业务Plugin
        -> report_runtime_events
        -> ServerEvent::AgentMessage或ServerEvent::AgentFailure

边界：
    daemon不读取Workspace YAML、不编译资源、不执行工具、不检查资源可见性、不实现LLM推理
    daemon只在网络边界解析协议并将逻辑Workspace/Agent身份映射为内部Entity
    当前客户端路由失败没有独立错误事件，统一通过结构化日志报告
```

## 持有关系

```text
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
```
