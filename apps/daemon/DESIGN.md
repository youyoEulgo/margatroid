# MargatroidDaemon

## 类型

私有：
```text
DaemonConfig：daemon启动配置，私有结构体
    bind: SocketAddr--ServerPlugin监听地址
    data_root: PathBuf--AgentImage和模型配置根目录
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
    启动daemon：创建数据目录，安装运行时、业务Plugin和ApiPlugin，启动日志转发并运行App
    行为：
        使用data_root/skills打开SkillPlugin
        使用data_root/workflows打开WorkflowPlugin
        两个工具定义Plugin必须在ToolPlugin之后、AgentPlugin之前安装

install_log_forwarder(app: &App) -> Result<(), Error>
    安装日志转发：取得LogPlugin的TracingStream和RuntimeEventSender，提交异步服务

forward_logs(stream: TracingStream, events: RuntimeEventSender)
    转发日志：订阅结构化Tracing记录，转换为ServerEvent::Log并发送Broadcast目标的WebSocketMessageSend

handle_workspace_start_requests(world: &mut World)
    处理Workspace启动请求：读取WorkspaceStartRequested并恢复WorkspaceDefinition
    行为：
        调用WorkspaceDefinitionDto::into_definition
        成功时发送StartWorkspace
        失败时写warning日志，不发送StartWorkspace

handle_agent_message_requests(world: &mut World)
    处理Agent消息请求：读取AgentMessageRequested并路由内部AgentMessage
    行为：调用route_agent_message完成校验、Workspace查找、Agent或manager查找和事件发送

report_runtime_events(world: &mut World)
    报告运行事件：记录Server生命周期与Workspace失败，并发送Workspace和Agent API消息事件
    行为：
        StartWorkspaceResult成功时构造ServerEvent::WorkspaceStarted和WebSocketMessageSend
        margatroid_types::AgentMessage按Agent反查Workspace身份后构造ServerEvent::AgentMessage和WebSocketMessageSend
        AgentFailure按Agent反查Workspace身份，写warning日志并构造ServerEvent::AgentFailure和WebSocketMessageSend
        ServerStarted、ServerFailed、ServerStopped只写tracing日志

sync_frontend_state_system(world: &mut World)
    同步后端状态：私有无事件System，每次Runtime tick向webui类型连接发送后端权威状态
    行为：
        通过WorldWorkspaceExt::workspaces取得仍存活的Workspace Entity
        转换为WorkspaceInfoDto并按项目根和名称排序
        逐Workspace遍历WorkspaceAgents，调用AgentMemory::history_messages读取展示历史
        只转换history_messages中的User、Assistant和资源引用，不读取realtime_messages
        构造ServerEvent::StateSync
        使用WebSocketMessageTarget::Type("webui")构造WebSocketMessageSend
        发送WebSocketMessageSend，由ApiPlugin完成序列化和连接筛选
        空列表也照常发送，使客户端能够清除已经停止的Workspace
        任一Agent历史读取失败时记录warning并跳过本次完整快照，不发送残缺状态

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
    -> 安装AgentImageLoaderPlugin、InferencePlugin和ToolPlugin
    -> 安装SkillPlugin和WorkflowPlugin，注册skill与workflow Provider
    -> 安装MemoryPlugin、AgentPlugin和WorkspacePlugin
    -> 安装ApiPlugin，由ApiPlugin注册/ws和api_route_system
    -> 安装ConnectionPlugin，由ConnectionPlugin注册连接注册system
    -> 注册handle_workspace_start_requests和handle_agent_message_requests
    -> 注册report_runtime_events
    -> 注册sync_frontend_state_system
    -> 安装异步日志转发
    -> AppRunExt::run

客户端连接注册：
    ClientRequest::ConnectionRegister
        -> ApiPlugin::api_route_system
        -> ConnectionRegisterRequested
        -> ConnectionPlugin::connection_registration_system
        -> WebSocketConnections::set_connection_type
        -> 使用client_type和connection_id生成名称
        -> WebSocketConnections::set_name

客户端Workspace启动：
    WebSocketMessageReceived
        -> ApiPlugin::api_route_system
        -> WorkspaceStartRequested
        -> handle_workspace_start_requests
        -> WorkspaceDefinitionDto::into_definition
        -> StartWorkspace
        -> WorkspacePlugin创建Workspace和Agent
        -> StartWorkspaceResult
        -> report_runtime_events
        -> WebSocketMessageSend { ServerEvent::WorkspaceStarted }
        -> ApiPlugin::api_route_system
        -> 前端

客户端Agent消息：
    WebSocketMessageReceived
        -> ApiPlugin::api_route_system
        -> AgentMessageRequested
        -> handle_agent_message_requests
        -> WorldWorkspaceExt::workspace
        -> WorldWorkspaceExt::workspace_agent或workspace_manager
        -> margatroid_types::AgentMessage
        -> AgentPlugin和后续业务Plugin
        -> report_runtime_events
        -> WebSocketMessageSend { ServerEvent::AgentMessage或ServerEvent::AgentFailure }
        -> ApiPlugin::api_route_system
        -> 前端

客户端状态同步：
    每次Runtime tick
        -> sync_frontend_state_system
        -> 读取当前Workspace和各Agent history_messages
        -> WebSocketMessageSend { Type("webui"), ServerEvent::StateSync }
        -> ApiPlugin::api_route_system
        -> webui客户端整体替换业务视图
    realtime_messages不进入跨进程协议

边界：
    daemon不读取Workspace YAML、不编译资源、不执行工具、不检查资源可见性、不实现LLM推理
    daemon不解析或序列化WebSocket API JSON，不直接取得WebSocketSender发送消息
    daemon负责消费ApiPlugin请求事件、生成连接名称并将逻辑Workspace/Agent身份映射为内部Entity
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
├── ApiPlugin
├── ConnectionPlugin
├── AgentImageLoaderPlugin
├── InferencePlugin
├── ToolPlugin
├── SkillPlugin
├── WorkflowPlugin
├── MemoryPlugin
├── AgentPlugin
└── WorkspacePlugin
```
