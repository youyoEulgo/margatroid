# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# DtoPlugin

## 类型

公开：
```text
DtoPlugin：WebSocket DTO转换插件，公开结构体--注册WebSocket路由并在协议DTO与领域事件之间转换
    websocket_path: String--WebSocket路由路径，默认/ws
    schedule: String--dto_route_system所属Runtime schedule，默认RuntimePlugin::UPDATE
    new() -> Self
        构造插件：使用默认路径和Schedule
    with_websocket_path(self, path: impl Into<String>) -> Self
        设置路径：保存调用者路径并返回自身
    with_schedule(self, schedule: impl Into<String>) -> Self
        设置Schedule：保存调用者Schedule并返回自身
    impl Default for DtoPlugin
        Default：公开trait实现，与new等价
        default() -> Self
            构造默认插件：调用new
    impl Plugin for DtoPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：检查Server与TracingStream依赖，启动日志转发服务并注册WebSocket路由、collect_external_events_system和dto_route_system

WebSocketMessageTarget：发送目标，公开枚举--描述连接筛选方式
    Broadcast--全部连接
    Type(String)--指定connection_type的连接
    Name(String)--指定唯一连接名称

WebSocketMessageSend：发送事件，公开结构体--承载ServerMessage和连接筛选目标
    target: WebSocketMessageTarget--发送范围
    message: ServerMessage--待序列化协议事件
    impl Event for WebSocketMessageSend
        Event：公开trait实现

```

私有：
```text
DtoPluginInstalled：DtoPlugin安装标记，私有单元Resource--阻止同一App重复安装
    impl Resource for DtoPluginInstalled
        Resource：私有trait实现

BackendStateReportCache：后端状态报告缓存，私有Resource--阻止未变化快照在事件驱动Runtime中形成自唤醒循环
    state: Option<BackendStateDto>--上次同步的完整状态
    recipients: Vec<u64>--上次报告时匹配backend_state目标的连接ID
    last_error: Option<String>--最近一次转换错误，用于相同错误只记录一次
    impl Resource for BackendStateReportCache
        Resource：私有trait实现
```

## 函数

私有：
```text
dto_route_system(world: &mut World)
    路由WebSocket API：私有System，执行入站解包和出站序列化
    行为：
        WebSocketMessageReceived:
        收集WebSocketMessageReceived并要求消息为Text
        反序列化统一{type,id,message}信封为ClientMessage
        按type取得对应DTO并调用DTO::into_domain
        转换成功后直接发送StartWorkspace、StopWorkspaceByReference、RouteAgentMessage、RouteAgentTurnAbort、RouteAgentVisibility或RegisterConnection领域事件
        转换失败时记录warning并丢弃当前请求

        WebSocketMessageSend:
        收集WebSocketMessageSend
        将ServerMessage序列化为Text
        直接使用事件携带的target，按Broadcast、Type或Name取得Vec<WebSocketSender>
        使用发送器集合和Text消息构造WebSocketMessageSender并调用try_send

collect_external_events_system(world: &mut World)
    收集外部事件：私有System，将允许发送给外部的领域事件转换为ServerMessage并包装成WebSocketMessageSend
    行为：
        将Server生命周期、WebSocket连接、断开和协议失败写入结构化日志
        收集StartWorkspaceResult、StopWorkspaceByReferenceResult、AgentMessage和AgentFailure
        AgentCreateResult、AgentVisibleResourceInjected、AgentVisibleResourceRemoved和AgentVisibleResourceInjectionFailed不单独生成API消息；它们通过BackendStateDto和结构化日志体现
        调用Protocol的FromDomain实现解析Workspace和Agent逻辑身份
        构造对应WorkspaceStarted、WorkspaceStartFailed、WorkspaceStopped、WorkspaceStopFailed、AgentMessage或AgentFailure协议事件
        记录Workspace启停、用户消息路由、Assistant响应和Agent失败等业务日志
        Workspace启停结果及AgentFailure选择logs目标，AgentMessage选择member_messages目标
        发送WebSocketMessageSend
        调用()::into_dto(&World)构造包含Workspace定义、每个Agent的Creating/Ready/Failed状态、Ready Agent动态可见性、累计Token状态和历史的BackendStateDto
        状态内容或backend_state接收连接集合变化时才发送StateSync
        相同状态转换错误只记录一次，成功后清除错误缓存
        将StateSync发送给backend_state指定目标

report_server_events(world: &World)
    报告Server事件：私有函数，将Server启停、WebSocket连接、断开和协议失败投影为结构化日志

report_workspace_events(world: &World, targets: &[WebSocketMessageTarget])
    报告Workspace启动：私有函数，成功时转换并发送WorkspaceStarted，失败时写错误日志并发送WorkspaceStartFailed

report_workspace_stop_events(world: &World, targets: &[WebSocketMessageTarget])
    报告Workspace停止：私有函数，发送WorkspaceStopped或WorkspaceStopFailed并记录日志

report_agent_messages(world: &World, targets: &[WebSocketMessageTarget])
    报告Agent消息：私有函数，将外部可见User和Assistant转换为AgentMessage协议事件；跳过System和Tool

report_agent_failures(world: &World, targets: &[WebSocketMessageTarget])
    报告Agent失败：私有函数，将AgentFailure转换为协议事件并记录warning

report_backend_state(world: &mut World, targets: &[WebSocketMessageTarget])
    报告后端状态：私有函数，从World构造权威状态快照并在快照或接收连接变化时发送StateSync
    调用：collect_external_events_system每次运行时调用
    输出：仅在首次运行、BackendStateDto变化或backend_state接收连接集合变化时发送WebSocketMessageSend

send_to_targets(world: &World, targets: &[WebSocketMessageTarget], message: ServerMessage)
    按目标发事件：私有函数，为每个目标克隆ServerMessage并发送一个WebSocketMessageSend

target_recipients(connections: &WebSocketConnections, targets: &[WebSocketMessageTarget]) -> Vec<u64>
    解析接收连接：私有函数，按目标查询发送器并返回连接ID；排序和去重由调用方完成

forward_logs(stream: TracingStream, events: RuntimeEventSender, targets: Vec<WebSocketMessageTarget>)
    转发结构化日志：私有异步函数，订阅TracingStream，将TracingRecord转换为LogRecordDto并按logs目标发送WebSocketMessageSend
    行为：订阅滞后时直接累计TracingSubscription内部丢弃计数，不向正在消费的TracingStream写回日志

```

## 逻辑

```text
入站：
    WebSocketMessageReceived
        -> ClientMessage { type, id, message }
        -> message DTO::into_domain
        -> 直接发送领域事件

出站：
    外部可见领域事件
        -> collect_external_events_system构造ServerMessage
        -> 包装WebSocketMessageSend
    Workspace、Agent和Memory当前状态
        -> collect_external_events_system构造BackendStateDto并与缓存比较
        -> 内容或接收连接集合未变化时结束
        -> StateSync发送给backend_state目标
    LogPlugin::TracingStream
        -> forward_logs异步订阅
        -> LogRecordDto转换
        -> 按logs目标发送WebSocketMessageSend
    其他Plugin也可直接发送WebSocketMessageSend
        -> DtoPlugin序列化ServerMessage
        -> WebSocketConnections按target筛选
        -> 构造WebSocketMessageSender并发送Text
```

## 边界

```text
DtoPlugin负责：WebSocket帧、JSON信封、DTO转换调用、入站领域事件发送、外部可见领域事件投影、完整状态快照、普通出站消息的发送终端构造、结构化日志转发、ServerMessage序列化和连接筛选
DtoPlugin不负责：manager路由、Agent Entity创建、Memory、Tool、Inference和资源权限
Protocol负责：XxxDto及其IntoDomain/FromDomain转换，包括使用World进行只读身份投影
LogPlugin负责：生成TracingStream，不感知DTO、WebSocket或连接目标
WorkspacePlugin负责：消费Workspace领域命令、解析Workspace和Agent逻辑身份
AgentPlugin负责：处理已经携带Entity的AgentMessage
```
