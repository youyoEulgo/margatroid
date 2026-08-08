# ApiPlugin

## 类型

公开：
```text
ApiPlugin：API路由插件，公开结构体--注册WebSocket API路由并双向转换API消息与内部事件
    websocket_path: String--WebSocket路由路径，默认/ws
    schedule: String--api_route_system所在的Runtime schedule，默认RuntimePlugin::UPDATE
    new() -> Self
        构造API插件：公开方法
        行为：使用/ws和RuntimePlugin::UPDATE构造插件
    with_websocket_path(path: impl Into<String>) -> Self
        设置WebSocket路径：公开方法
        行为：保存调用者提供的路径并返回自身
    with_schedule(schedule: impl Into<String>) -> Self
        设置执行schedule：公开方法
        行为：保存调用者提供的schedule名称并返回自身
    impl Default for ApiPlugin
        Default：公开trait实现
        default() -> Self
            构造默认API插件：使用new返回默认配置
    impl Plugin for ApiPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装API插件：注册路由、事件和唯一的API路由system
            行为：
                检查ApiPlugin没有重复安装
                检查schedule存在
                使用websocket_path注册ServerPlugin的WebSocket事件路由
                注册ConnectionRegisterRequested、WorkspaceStartRequested、AgentMessageRequested和WebSocketMessageSend事件
                将api_route_system加入schedule

ConnectionRegisterRequested：连接注册API请求，公开事件--保存客户端声明的连接类型和来源连接ID
    connection_id: WebSocketConnectionId--来源WebSocket连接
    client_type: String--客户端声明的类型，例如webui或cli
    impl Event for ConnectionRegisterRequested
        Event：公开trait实现

WorkspaceStartRequested：Workspace启动API请求，公开事件--原样保存客户端workspace.start请求供其他system处理
    connection_id: WebSocketConnectionId--来源WebSocket连接
    id: String--客户端生成的请求ID
    definition: WorkspaceDefinitionDto--客户端提交的Workspace定义DTO
    impl Event for WorkspaceStartRequested
        Event：公开trait实现

AgentMessageRequested：Agent消息API请求，公开事件--原样保存客户端agent.message请求供其他system处理
    connection_id: WebSocketConnectionId--来源WebSocket连接
    id: String--客户端生成的消息ID
    workspace: WorkspaceRefDto--目标Workspace逻辑引用
    agent: Option<String>--可选目标Agent名称，None的含义由后续system决定
    content: String--客户端提交的消息正文
    impl Event for AgentMessageRequested
        Event：公开trait实现

WebSocketMessageTarget：WebSocket消息目标，公开枚举--描述api_route_system应选择哪些连接
    Broadcast--全部当前连接
    Type(String)--connection_type与给定值相同的全部连接
    Name(String)--具有给定唯一名称的单个连接

WebSocketMessageSend：WebSocket消息发送请求，公开事件--统一承载后端发给客户端的协议事件
    target: WebSocketMessageTarget--连接筛选方式
    message: ServerEvent--待序列化的后端协议事件
    impl Event for WebSocketMessageSend
        Event：公开trait实现
```

私有：
```text
ApiPluginInstalled：API插件安装标记，私有资源--防止同一个App重复安装ApiPlugin
```

## 函数

私有：
```text
api_route_system(world: &mut World)
    路由API消息：私有system，在WebSocket消息与内部API事件之间双向转换
    行为：
        收集当前事件队列中的全部WebSocketMessageReceived
        逐条处理收到的消息
            检查WebSocket消息是否为Text
                不是Text时写warning日志并结束当前消息
            将Text反序列化为margatroid_protocol::ClientRequest
                反序列化失败时写warning日志并结束当前消息
            根据ClientRequest的type分支
                ConnectionRegister：
                    使用received.connection_id和client_type构造ConnectionRegisterRequested
                    发送ConnectionRegisterRequested
                WorkspaceStart：
                    使用received.connection_id、id和definition构造WorkspaceStartRequested
                    发送WorkspaceStartRequested
                AgentMessage：
                    使用received.connection_id、id、workspace、agent和content构造AgentMessageRequested
                    发送AgentMessageRequested
        收集当前事件队列中的全部WebSocketMessageSend
        逐条处理待发送消息
            将message序列化为JSON文本
                序列化失败时写warning日志并结束当前消息
            将JSON文本包装为WebSocketMessage::Text
            根据target从WebSocketConnections筛选发送器
                Broadcast调用get_all
                Type调用get_by_type
                Name调用get_by_name
            对筛选出的每个发送器调用try_send
            单个连接发送失败时写warning日志，不影响其他连接
```

## 逻辑

```text
前端到后端：
    WebSocket Text frame
        -> ServerPlugin::WebSocketMessageReceived
        -> ApiPlugin::api_route_system
        -> margatroid_protocol::ClientRequest
        -> 按type发送一种内部API事件
            connection.register -> ConnectionRegisterRequested
            workspace.start -> WorkspaceStartRequested
            agent.message -> AgentMessageRequested
```

```text
后端到前端：
    其他system构造margatroid_protocol::ServerEvent
        -> WebSocketMessageSend
        -> ApiPlugin::api_route_system
        -> serde_json序列化
        -> WebSocketMessage::Text
        -> WebSocketConnections按Broadcast、Type或Name筛选
        -> WebSocketSender::try_send
```

```text
错误处理：
    非Text消息、无法反序列化的ClientRequest和无法序列化的ServerEvent
        -> 写warning日志
        -> 丢弃当前消息
    找不到Name目标时不发送消息并写warning日志
    Type目标没有匹配连接时不发送消息
    请求字段语义是否合法由消费对应请求事件的system判断
```

## 边界

```text
ApiPlugin负责：
    WebSocket API路由注册
    WebSocketMessageReceived到ClientRequest的反序列化
    按ClientRequest类型包装并发送对应内部API事件
    WebSocketMessageSend中ServerEvent的序列化
    按连接目标筛选WebSocketSender并发送Text消息

ApiPlugin不负责：
    校验client_type或为连接生成名称
    修改WebSocketConnections中的连接类型和名称
    WorkspaceDefinitionDto到WorkspaceDefinition的转换
    请求字段的语义校验
    Workspace和Agent逻辑身份到Entity的解析
    manager路由
    生成ServerEvent承载的业务数据，由ApiIntegrationPlugin负责
    Workspace、Agent、Memory、Inference、Tool、Skill和Workflow业务
    读取Workspace文件、SQLite或任何资源正文
    流式WebSocket消息
```
