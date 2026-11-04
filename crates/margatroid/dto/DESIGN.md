Margatroid领域实现契约；本领域已切换，mecs现有实现保持不动，后续另行清理。

# 伪代码格式

```text
板块：一级标题，一行注明实际文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列字段及用途；函数列参数、结果和直接行为。
System列处理事件、接收闭包结果、提交闭包及产出事件；空项使用None。
属性不写Rust Attribute；持有关系使用树形图。
DTO可依赖Margatroid基础设施axum_plugin注册Router；业务plugin之间仍不建立依赖。
```

# 板块格式

```text
# lib     dto/plugin/src/lib.rs      安装、自有Router及连接入口
# system  dto/plugin/src/system.rs   已解析DTO、领域结果及出站路由
# handler dto/plugin/src/handler.rs  原生handler、连接读写、领域转换
# types   dto/plugin/src/types.rs    连接状态、请求关联与投影缓存
```

# lib

## 类型

公开：
```text
DtoPlugin：网络协议边界插件
    impl Plugin for DtoPlugin
        build(self, app: &mut App)
            行为：
                检查运行设施、AxumHandle及MargatroidConfig，拒绝重复安装。
                创建DtoState、DTO owned ConnState、请求关联表和投影缓存。
                从AxumHandle.lifetime()取得共享AxumLifetime；DTO只使用其退出信号及升级Future跟踪。
                构造Router::new().route("/ws", get(ws_handler)).with_state(state)。
                调用AppAxumExt::add_axum_routes(Router)，不要求Compose手工添加路由。
                在PRE_UPDATE安装abort_control_system，在UPDATE安装DTO请求及结果System。
                日志订阅通过send_async_closure提交DTO自有owned读取闭包，产生LogReady并监听退出，完成载荷交DTO System收口，不调用spawn_async_service。
```

# system

## 函数

crate公开：
```text
dto_route_system(world: &mut World)
    处理事件：DtoRequest、ClientDisconnected、WebSocketMessageSend
    接收闭包结果：None
    提交闭包：wait_mcl_reply(MclReplyContext) -> Future<Output = Result<MclReplyOutput, MclReplyError>>
    产出事件：RegisterClient、MclCommand、StartWorkspace、StopWorkspace、RestartWorkspace、AbortTurnControl
    行为：DtoRequest -> handle_dto_request；ClientDisconnected -> handle_disconnect；WebSocketMessageSend -> handle_outbound_message。

collect_external_events_system(world: &mut World)
    处理事件：ClientRegistrationResult、StartWorkspaceResult、StopWorkspaceResult、RestartWorkspaceResult、LogReady、AgentException
    接收闭包结果：Result<HistoryOutput, HistoryError>、Result<MclReplyOutput, MclReplyError>、SnapshotReady
    提交闭包：read_history(HistoryContext) -> Future<Output = Result<HistoryOutput, HistoryError>>；读取状态所需历史快照
    产出事件：WebSocketMessageSend
    行为：注册和生命周期结果 -> handle_domain_result；历史及MCL结果 -> 对应Handler；状态快照、日志与异常沿现有业务边界投影和发送。

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

私有，原生Axum入口：
```text
ws_handler(State(state): State<DtoState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Future<Output = Response>
    行为：
        提取owned IngressRequest，直接调用state.ingress.evaluate(&request)；失败直接返回HTTP拒绝响应。
        检查退出状态并配置现有协议消息上限；握手失败直接返回Response，不投递World事件。
        分配ConnectionId和有界mpsc<ClientFrame>队列，在ConnState保留真实连接与ClientSender。
        在on_upgrade前通过lifetime.track()取得AxumWorkGuard，再将guard和State移动到升级回调；回调收到socket后才构造serve_connection，guard持有到完整连接Future退出。
        只返回原生升级Response；升级失败清理预留条目、关闭接收端并发送一次ClientDisconnected。

serve_connection(state: DtoState, connection: ConnectionId, socket: WebSocket, outbound: mpsc::Receiver<ClientFrame>) -> Future<Output = ()>
    行为：
        DTO拥有socket拆分后的读写端，读循环解析wire，写循环消费有界出站队列。
        合法消息解析为ClientMessage；只对需要World的请求发送DtoRequest，不发送原始帧或统一网络事件。
        无效JSON、非法信封、类型或大小错误直接沿当前协议错误响应/关闭规则处理；保留1007/1009等关闭码，不经过ECS。
        不合成默认流事件；inference发送的JSON文本帧原样写入，不新增wire envelope。
        写端将ClientFrame::Text/Binary/Close映射为Axum帧；Ping/Pong及协议控制由DTO处理，不进入Client core。
        读写均监听lifetime.cancelled()；任一结束则停止另一端，执行有界关闭并释放socket和接收端。
        终结时从ConnState移除条目并恰好一次发送ClientDisconnected，发送能力随接收端关闭失效。
        应用退出由AxumLifetime跟踪此完整Future；HTTP graceful结束之外还等待所有升级连接Future退出。
```

crate公开：
```text
handle_dto_request(world: &mut World, request: DtoRequest)
    行为：
        仅处理已解析协议值；先复检ConnState中的原连接仍存活，未注册连接只能注册。
        注册从ConnState取得可信ClientSender，生成RegisterClient；不接受wire注入连接身份或发送能力。
        其他请求从ConnectionId查询Client Entity，拒绝未注册连接及重复在途网络ID。
        运行目标经address解析为Entity，核对组件、Workspace归属及实例。
        Workspace列表同步投影内存配置及路径展示缓存；启动和重启只传path，后端重读定义；文件资源不创建占位Entity。
        回合中止发送AbortTurnControl，由下一帧控制Handler同步裁决，不在网络解析处直接取消。
        其他异步请求生成operation_id并保存网络关联，转换为既有领域请求；失败发送原关联协议错误。

handle_abort_control(world: &mut World, request: AbortTurnControl)
    行为：复检Client、Agent完整Entity、VM实例和turn；同步agent_abort_turn并请求取消对应回合任务；发送匹配网络id的受理或失败回执。

handle_mcl_reply(world: &mut World, result: Result<MclReplyOutput, MclReplyError>)
    行为：复检关联，将MclValue或错误转换为mcl.command_result；继续等待MclCommand原oneshot，不新增领域结果事件。

handle_history_result(world: &mut World, result: Result<HistoryOutput, HistoryError>)
    行为：核对原Client和目标实例，转换history.result或request.failed；保留既有历史查询、快照合并和状态发布边界。

handle_domain_result(world: &mut World, result: DomainResult)
    行为：
        按operation_id取关联，核对操作类型、原目标Entity及实例。
        复检原Client Entity和连接，转换既有protocol响应并只发原连接。
        注册结果在原连接仍存活时应答；已结束连接的迟到成功按ClientDisconnected契约清理，不产生遗留Client。
        完成后释放关联；重复、迟到或已断连结果不重新绑定目标。

handle_disconnect(world: &mut World, event: &ClientDisconnected)
    行为：清理连接的请求关联和投影接收状态；ClientPlugin独立处理同一事件销毁身份；不撤销已受理领域操作。

prepare_backend_state(world: &World) -> Result<BackendStateDto, ProtocolErrorDto>
    行为：读取领域core组件与已加载内存配置，从Address取得对外地址；保留状态、历史及异常的既有协议字段，不输出凭据、源码或内部句柄。

handle_outbound_message(world: &mut World, message: WebSocketMessageSend)
    行为：按出站配置从已注册Client组件筛选目标，序列化ServerMessage或既有JSON值为ClientFrame::Text，通过ClientSender::try_send发送；原连接注册回执可使用ConnState中的sender。
    行为：Full/Closed立即处理，不为每次发送启动任务；拥塞关闭由DTO取消该连接任务，保持有界内存及既有失败语义。
```

私有：
```text
wait_mcl_reply(context: MclReplyContext) -> Future<Output = Result<MclReplyOutput, MclReplyError>>
    行为：等待原oneshot，携带原上下文返回，不使用通用后台服务注册。

read_history(context: HistoryContext) -> Future<Output = Result<HistoryOutput, HistoryError>>
    行为：异步读取历史，返回原请求上下文及成功或失败载荷。

resolve_live_target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
    行为：解析唯一Entity，检查领域组件、归属和实例。

convert_request(message: ClientMessage, context: RequestContext) -> Result<DomainRequest, ProtocolErrorDto>
    行为：将纯协议值转换为领域core事件，内存对象关联只使用Entity。

convert_result(result: DomainResult, context: RequestContext) -> Result<ServerMessage, ProtocolErrorDto>
    行为：映射既有领域结果、错误和关联信息，需要对外地址时读取Address。
```

# types

## 类型

公开：
```text
WebSocketMessageSend：DTO出站协议事件
    target: OutboundTarget
    message: serde_json::Value--保持既有出站JSON形状
    impl Event + Clone + Debug
    行为：只表达DTO协议出站请求，不是通用传输事件；发送能力本身由client_core提供。
```

私有：
```text
DtoState：原生handler共享状态，Clone
    ingress: IngressPolicy--配置快照
    events: RuntimeEventSender--只投递DTO已解析事件及Client领域事件
    connections: Arc<ConnState>--DTO连接状态，同时供World侧DTO Handler查询
    lifetime: AxumLifetime--基础设施退出信号及升级Future跟踪

ConnState：DTO owned连接索引
    entries: Mutex<BTreeMap<ConnectionId, ConnectionEntry>>--包含未注册连接
    next_id: AtomicU64--进程内不复用，溢出拒绝新连接
    行为：有界分配出站队列、查询sender、取消拥塞连接及幂等移除；只DTO维护，不放进Client core。

ConnectionEntry：真实连接的边界材料
    sender: ClientSender
    cancel: CancellationToken--单连接退出信号
    行为：不保存第二份注册身份；Client实体拥有client_type/name。

DtoRequest：已解析入站请求，DTO私有事件
    connection: ConnectionId--handler绑定，不信任wire
    message: ClientMessage--已完成协议解析，不含Axum帧
    impl Event + Clone + Debug

RequestContext：边界请求上下文
    network_id: String
    operation_id: String
    client: Option<Entity>--注册前为None，其余为原完整实例
    connection: ConnectionId
    target: OperationTarget

OperationTarget：操作关联，枚举
    Live { workspace: Option<Entity>, target: Entity, instance: Option<String> }
    Discovery
    Registration

PendingRequests：在途关联表，Resource
    entries: BTreeMap<String, RequestContext>--operation_id为键
    duplicate(&self, client: Entity, network_id: &str) -> bool
    remove_connection(&mut self, connection: ConnectionId)
        行为：删除连接关联，不触发业务取消。

AbortTurnControl：DTO控制事件
    context: RequestContext
    agent: Entity
    instance: String
    turn: String
    impl Event

MclReplyContext：RequestContext及MCL oneshot接收端
MclReplyOutput / MclReplyError：原上下文及MclValue或协议错误
HistoryContext：RequestContext及owned历史读取句柄
HistoryOutput / HistoryError：原上下文及历史记录或协议错误
SnapshotReady：既有状态历史快照完成载荷，保留token关联与合并规则
LogReady：已转换的日志协议记录，DTO私有事件
DomainRequest / DomainResult：对应领域core请求和结果，保留operation_id
BackendStateReportCache：投影缓存，Resource；保存投影、接收者及刷新关联，不保存第二份领域运行状态
```

# 持有关系

```text
App
├── axum_plugin（Margatroid基础设施）
│   ├── 合并各plugin的原生Router
│   └── AxumLifetime（退出信号、连接Future退出跟踪）
└── World
    ├── DtoState -> Arc<ConnState>
    │   └── ConnectionId -> ConnectionEntry（含未注册连接）
    ├── PendingRequests -> RequestContext
    ├── BackendStateReportCache
    └── 已注册Client -> ClientSender

DTO升级连接Future（由AxumLifetime跟踪）
├── socket读写端
├── 有界mpsc<ClientFrame>接收端
└── 终结 -> 关闭能力、删除连接条目、ClientDisconnected

DtoPlugin
├── axum_plugin（仅基础设施依赖例外）及原生Axum
├── protocol、address、config_core
└── 各领域core（ClientSender、组件、请求、结果）
```
