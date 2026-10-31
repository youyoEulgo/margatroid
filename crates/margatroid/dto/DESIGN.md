# 伪代码格式

```text
板块：一级标题，一行注明实际文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列出字段及用途；函数列出参数、结果和直接行为。
System分别列出处理事件、接收闭包结果、提交闭包及产出事件，使用Rust类型和函数名；空项使用None。
属性不写Rust Attribute；持有关系使用树形图。
```

# 板块格式

```text
# lib     src/lib.rs      Plugin安装与公开入口
# system  src/system.rs   网络、领域结果及出站事件路由
# handler src/handler.rs  身份绑定、寻址、领域转换与收发
# types   src/types.rs    请求关联、历史结果与投影缓存
```

# lib

## 类型

公开：
```text
DtoPlugin：网络边界插件，公开类型
    impl Plugin for DtoPlugin
        build(self, app: &mut App)
            行为：检查运行、连接和出站配置设施；初始化关联表与投影缓存；在PRE_UPDATE安装abort_control_system，在UPDATE安装请求及结果System；接入现有网络事件路由。
```

# system

## 函数

crate公开：
```text
dto_route_system(world: &mut World)
    处理事件：WebSocketMessageReceived、WebSocketDisconnected、WebSocketMessageSend
    接收闭包结果：None
    提交闭包：wait_mcl_reply(MclReplyContext) -> Future<Output = Result<MclReplyOutput, MclReplyError>>
    产出事件：RegisterConnection、MclCommand、StartWorkspace、StopWorkspace、RestartWorkspace、AbortTurnControl
    行为：WebSocketMessageReceived -> handle_inbound_message；WebSocketDisconnected -> handle_disconnect；WebSocketMessageSend -> handle_outbound_message。

collect_external_events_system(world: &mut World)
    处理事件：ServerStarted、ServerFailed、ServerStopped、WebSocketConnected、WebSocketProtocolFailed、ClientRegistrationResult、StartWorkspaceResult、StopWorkspaceResult、RestartWorkspaceResult
    接收闭包结果：Result<HistoryOutput, HistoryError>、Result<MclReplyOutput, MclReplyError>
    提交闭包：read_history(HistoryContext) -> Future<Output = Result<HistoryOutput, HistoryError>>
    产出事件：WebSocketMessageSend
    行为：ClientRegistrationResult、StartWorkspaceResult、StopWorkspaceResult、RestartWorkspaceResult -> handle_domain_result；Result<HistoryOutput, HistoryError> -> handle_history_result；Result<MclReplyOutput, MclReplyError> -> handle_mcl_reply；ServerStarted、ServerFailed、ServerStopped、WebSocketConnected、WebSocketProtocolFailed -> handle_transport_event。


abort_control_system(world: &mut World)
    处理事件：AbortTurnControl
    接收闭包结果：None
    提交闭包：None
    产出事件：WebSocketMessageSend
    oneshot回复：None
    行为：AbortTurnControl -> handle_abort_control。

```

# handler

## 函数

crate公开：
```text
handle_inbound_message(world: &mut World, connection: WebSocketConnectionId, message: WebSocketMessage)
    行为：
        检查消息上限并按protocol解析信封。
        除注册外，从真实连接查询Client Entity；拒绝未注册连接及重复在途网络ID。
        运行目标经address解析为Entity，核对组件、Workspace归属及实例。
        Workspace列表同步投影路径与展示缓存；启动和重启只传path，后端重读定义；文件资源不创建占位Entity。
        列出Workspace直接投影内存配置并回复，不发领域事件或提交异步任务。
        回合中止发送AbortTurnControl，下一帧控制Handler同步裁决，不在网络解析Handler直接取消。
        其他异步请求生成operation_id，保存网络关联，转换为领域请求并发送；失败返回关联协议错误。

handle_abort_control(world: &mut World, request: AbortTurnControl)
    行为：复检Client、Agent完整Entity、VM实例和turn；同步agent_abort_turn并请求取消对应回合的任务；发送匹配网络id的受理或失败回执。

handle_mcl_reply(world: &mut World, result: Result<MclReplyOutput, MclReplyError>)
    行为：复检请求关联并将MclValue或错误转换为mcl.command_result，不读取MclCommandResult事件。

handle_history_result(world: &mut World, result: Result<HistoryOutput, HistoryError>)
    行为：按原请求关联核对Client和目标实例，转换history.result或request.failed，发送WebSocketMessageSend。

handle_transport_event(world: &mut World, event: TransportEvent)
    行为：处理连接和Server通知，准备状态或错误投影并发送WebSocketMessageSend。

handle_domain_result(world: &mut World, result: DomainResult)
    行为：
        按operation_id取请求关联，核对操作类型、原目标Entity及实例。
        复检原Client Entity和连接；转换为protocol响应并只发原连接。
        完成后释放关联；重复、迟到或已断连结果不重新绑定目标。

handle_disconnect(world: &mut World, event: &WebSocketDisconnected)
    行为：清理该连接的请求关联和发送状态，不撤销已受理领域操作。

prepare_backend_state(world: &World) -> Result<BackendStateDto, ProtocolErrorDto>
    行为：读取领域core公开组件与已加载内存配置；从Address取得对外地址；生成不含凭据、源码及内部句柄的协议投影。

handle_outbound_message(world: &mut World, message: PreparedMessage)
    行为：按目标连接及出站配置发送protocol消息。
```

私有：
```text
wait_mcl_reply(context: MclReplyContext) -> Future<Output = Result<MclReplyOutput, MclReplyError>>
    行为：等待MclCommand原oneshot，不新增领域结果事件。

read_history(context: HistoryContext) -> Future<Output = Result<HistoryOutput, HistoryError>>
    行为：异步读取历史，返回携带原请求上下文的成功或失败载荷。

resolve_live_target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
    行为：通过address解析唯一Entity；检查所需领域组件、归属和实例。

convert_request(message: ClientMessage, context: RequestContext) -> Result<DomainRequest, ProtocolErrorDto>
    行为：将纯协议值转换为领域core事件；内存对象关联只使用Entity。

convert_result(result: DomainResult, context: RequestContext) -> Result<ServerMessage, ProtocolErrorDto>
    行为：映射领域结果、错误及关联信息；需要对外地址时从Address读取。
```

# types

## 类型

私有：
```text
RequestContext：边界请求上下文
    network_id: String--原网络请求ID
    operation_id: String--内部唯一操作ID
    client: Entity--原Client完整实例
    connection: WebSocketConnectionId--原连接
    target: OperationTarget--操作目标

OperationTarget：目标关联，私有枚举
    Live { workspace: Option<Entity>, target: Entity, instance: Option<String> }
    Discovery
    Registration

PendingRequests：在途关联表
    entries: BTreeMap<String, RequestContext>--operation_id为键
    duplicate(&self, client: Entity, network_id: &str) -> bool
        行为：检查同一Client尚未结束的网络ID。
    remove_connection(&mut self, connection: WebSocketConnectionId)
        行为：删除连接关联，不触发业务取消。

AbortTurnControl：DTO私有控制事件
    context: RequestContext
    agent: Entity
    instance: String
    turn: String
    impl Event for AbortTurnControl

MclReplyContext：等待材料--RequestContext及MCL oneshot接收端
MclReplyOutput / MclReplyError：等待结果--原上下文及MclValue或协议错误

HistoryContext：历史读取上下文--RequestContext及owned历史读取句柄
HistoryOutput：历史成功载荷--HistoryContext及历史记录
HistoryError：历史失败载荷--HistoryContext及ProtocolErrorDto
TransportEvent：网络通知，私有枚举
    Started(ServerStarted)
    Failed(ServerFailed)
    Stopped(ServerStopped)
    Connected(WebSocketConnected)
    ProtocolFailed(WebSocketProtocolFailed)

DomainRequest：领域请求，私有枚举--包装对应core的注册、生命周期、MCL及查询请求
DomainResult：领域结果，私有枚举--包装对应core结果；各结果保留operation_id
PreparedMessage：待发送消息，私有类型--保存发送目标和ServerMessage
BackendStateReportCache：投影缓存，私有类型--缓存最近出站投影及接收者，不保存第二份领域运行状态
```

# 持有关系

```text
App
└── World
    ├── PendingRequests
    │   └── RequestContext
    │       ├── Client Entity（关联）
    │       ├── connection_id
    │       └── OperationTarget（关联）
    └── BackendStateReportCache

DtoPlugin
├── protocol（纯网络类型）
├── address（边界寻址）
├── 各领域core（组件、请求及结果）
└── mecs（连接、事件及异步设施）
```
