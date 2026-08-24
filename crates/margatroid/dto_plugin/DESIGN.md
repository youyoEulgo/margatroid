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
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

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

# 板块格式

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        图书馆组件与 Plugin
# system     src/system.rs     System 函数
# handler    src/handler.rs    处理函数
# events     src/events.rs     事件类型
# types      src/types.rs      其余类型
# error      src/error.rs      Error 与公开错误分类
```

# lib

lib 放 Plugin、安装标记 Resource 和公开 re-export。

## 类型

公开：
```text
WebSocketMessageTarget：发送目标，公开枚举--由config_plugin重新导出
    Broadcast--全部连接
    Type(String)--指定connection_type的连接
    Name(String)--指定唯一连接名称

DtoPlugin：WebSocket DTO转换插件，公开结构体--注册WebSocket路由并在协议DTO与领域事件之间转换
    websocket_path: String--WebSocket路由路径，私有
    schedule: String--dto_route_system所属Runtime schedule，私有
    new() -> Self
        构造插件：公开关联函数，使用默认路径/ws和RuntimePlugin::UPDATE
    with_websocket_path(mut self, path: impl Into<String>) -> Self
        设置路径：公开构建方法
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开构建方法
    impl Default for DtoPlugin
        Default：公开trait实现，调用new
    impl Plugin for DtoPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                重复安装时panic
                Schedule不存在时panic
                ServerPlugin未安装WebSocketConnections时panic
                读取MargatroidConfig和TracingStream，启动forward_logs异步日志转发
                插入DtoPluginInstalled、BackendStateReportCache和PendingMclCommandResponses
                注册WebSocket事件路由，并在schedule挂载collect_external_events_system和dto_route_system
```

私有：
```text
DtoPluginInstalled：DtoPlugin安装标记，私有单元Resource--阻止同一App重复安装
    impl Resource for DtoPluginInstalled
```

# system

system 放 System 函数。System 只读取本帧事件并克隆，然后调用 handler。

## 函数

crate公开：
```text
dto_route_system(world: &mut World)
    路由WebSocket API：crate公开System
    处理事件：WebSocketMessageReceived、WebSocketMessageSend
    行为：
        克隆本帧全部WebSocketMessageReceived，逐个调用handle_inbound_message
        调用handle_pending_mcl_responses处理已完成的MCL命令
        克隆本帧全部WebSocketMessageSend，调用handle_outbound_messages

collect_external_events_system(world: &mut World)
    收集外部事件：crate公开System
    行为：调用handle_collect_external_events
```

# handler

handler 放处理函数。入站解包、出站序列化、外部事件投影和日志转发都在此展开。

## 函数

crate公开：
```text
handle_inbound_message(world: &mut World, connection_id: WebSocketConnectionId, message: WebSocketMessage)
    处理入站消息：crate公开函数
    行为：
        要求消息为Text
        反序列化统一{type,id,message}信封为ClientMessage
        按type调用对应DTO::into_domain并发送领域事件
        MCL命令转换成功后把响应接收器写入PendingMclCommandResponses
        转换失败时写warn日志并丢弃当前请求

handle_pending_mcl_responses(world: &mut World)
    处理MCL响应：crate公开函数
    行为：
        读取WebSocketConnections
        逐个尝试接收PendingMclCommandResponses
        已完成的响应序列化为MclCommandResult并发送到原连接
        Empty保留，Disconnected移除

handle_outbound_messages(world: &mut World, outgoing: Vec<WebSocketMessageSend>)
    处理出站消息：crate公开函数
    行为：
        读取WebSocketConnections
        逐条序列化ServerMessage为Text
        按Broadcast、Type或Name解析发送器集合
        调用WebSocketMessageSender::try_send；Log消息发送失败不写warn

handle_collect_external_events(world: &mut World)
    收集外部事件：crate公开函数
    行为：
        读取MargatroidConfig目标
        依次调用report_server_events、report_workspace_events、report_workspace_stop_events、report_agent_messages、report_agent_failures和report_backend_state

forward_logs(stream: TracingStream, events: RuntimeEventSender, targets: Vec<WebSocketMessageTarget>)
    转发结构化日志：crate公开异步函数
    行为：
        订阅TracingStream
        把TracingRecord转换为LogRecordDto并按logs目标发送WebSocketMessageSend
        Lagged时继续，Closed时结束
```

私有：
```text
report_server_events(world: &World)
    报告Server事件：私有函数，将Server启停、WebSocket连接、断开和协议失败投影为结构化日志

report_workspace_events(world: &World, targets: &[WebSocketMessageTarget])
    报告Workspace启动：私有函数，成功时发送WorkspaceStarted，失败时写error日志并发送WorkspaceStartFailed

report_workspace_stop_events(world: &World, targets: &[WebSocketMessageTarget])
    报告Workspace停止：私有函数，发送WorkspaceStopped或WorkspaceStopFailed并记录日志

report_agent_messages(world: &World, targets: &[WebSocketMessageTarget])
    报告Agent消息：私有函数，将外部可见User和Assistant转换为AgentMessage协议事件；跳过System、Tool和Error；Error只通过历史状态同步渲染

report_agent_failures(world: &World, targets: &[WebSocketMessageTarget])
    报告Agent失败：私有函数，将AgentFailure转换为协议事件并记录warn

report_backend_state(world: &mut World, targets: &[WebSocketMessageTarget])
    报告后端状态：私有函数
    行为：
        构造BackendStateDto
        转换失败时相同错误只记录一次
        解析backend_state接收连接并排序去重
        仅在首次运行、快照变化或接收连接集合变化时发送StateSync

send_to_targets(world: &World, targets: &[WebSocketMessageTarget], message: ServerMessage)
    按目标发事件：私有函数，为每个目标克隆ServerMessage并发送一个WebSocketMessageSend

target_recipients(connections: &WebSocketConnections, targets: &[WebSocketMessageTarget]) -> Vec<u64>
    解析接收连接：私有函数，按目标查询发送器并返回连接ID；排序和去重由调用方完成
```

# events

events 放事件类型。

## 类型

公开：
```text
WebSocketMessageSend：发送事件，公开结构体--承载ServerMessage和连接筛选目标
    target: WebSocketMessageTarget--发送范围
    message: ServerMessage--待序列化协议事件
    impl Event for WebSocketMessageSend
```

# types

types 放除事件和错误外的其余类型。

## 类型

crate公开：
```text
BackendStateReportCache：后端状态报告缓存，crate公开Resource--阻止未变化快照在事件驱动Runtime中形成自唤醒循环
    state: Option<BackendStateDto>--上次同步的完整状态
    recipients: Vec<u64>--上次报告时匹配backend_state目标的连接ID
    last_error: Option<String>--最近一次转换错误，用于相同错误只记录一次
    impl Resource for BackendStateReportCache

PendingMclCommandResponses：MCL命令响应等待表，crate公开Resource--保存已发送MCL命令的一次性响应接收器
    commands: Vec<PendingMclCommandResponse>--等待中的MCL命令响应
    impl Resource for PendingMclCommandResponses

PendingMclCommandResponse：单条MCL命令响应，crate公开结构体
    id: String--请求ID
    connection_id: WebSocketConnectionId--来源连接
    response: Mutex<mpsc::Receiver<Result<serde_json::Value, String>>>--命令响应接收器
```

# error

```text
DtoPlugin 不定义错误类型；入站转换失败和出站发送失败只记录warn日志。
```

## 逻辑

```text
入站：
    WebSocketMessageReceived
        -> dto_route_system
        -> handle_inbound_message
        -> ClientMessage { type, id, message }
        -> message DTO::into_domain
        -> 直接发送领域事件

MCL响应：
    MclPlugin完成命令
        -> dto_route_system
        -> handle_pending_mcl_responses
        -> MclCommandResult发回原连接

出站：
    外部可见领域事件
        -> collect_external_events_system
        -> handle_collect_external_events构造ServerMessage
        -> 包装WebSocketMessageSend
    Workspace、Agent和Memory当前状态
        -> report_backend_state构造BackendStateDto并与缓存比较
        -> 内容或接收连接集合未变化时结束
        -> StateSync发送给backend_state目标
    LogPlugin::TracingStream
        -> forward_logs异步订阅
        -> LogRecordDto转换
        -> 按logs目标发送WebSocketMessageSend
    其他Plugin也可直接发送WebSocketMessageSend
        -> dto_route_system序列化ServerMessage
        -> WebSocketConnections按target筛选
        -> 构造WebSocketMessageSender并发送Text
```

## 边界

```text
DtoPlugin负责：WebSocket帧、JSON信封、DTO转换调用、入站领域事件发送、外部可见领域事件投影、完整状态快照、普通出站消息的发送终端构造、结构化日志转发、ServerMessage序列化和连接筛选。
DtoPlugin不负责：manager路由、Agent Entity创建、Memory、Tool、Inference和资源权限。
Protocol负责：XxxDto及其IntoDomain/FromDomain转换，包括使用World进行只读身份投影。
LogPlugin负责：生成TracingStream，不感知DTO、WebSocket或连接目标。
WorkspacePlugin负责：消费Workspace领域命令、解析Workspace和Agent逻辑身份。
AgentPlugin负责：处理已经携带Entity的AgentMessage。
```
