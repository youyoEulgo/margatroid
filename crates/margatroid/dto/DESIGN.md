# 伪代码格式

```text
板块使用一级标题，类型及函数使用二级标题。
类型列出实际Rust字段和持有关系，函数列出签名及直接行为。
System列出处理事件、闭包结果、产出事件及回复。
```

# 板块格式

```text
# lib      dto/plugin/src/lib.rs
# system   dto/plugin/src/system.rs
# handler  dto/plugin/src/handler.rs、projection.rs、ui.rs
# types    dto/plugin/src/types.rs
```

# lib

## 类型

公开：
```text
DtoPlugin
    new() -> Self
    impl Default
    impl Plugin
        build(self, app: &mut App)
            行为：
                检查RuntimeHandle、AsyncRuntimeHandle、AxumHandle及MargatroidConfig。
                初始化DtoState、ConnState及SyncState；SyncState包含按Agent持有的UI观察及发送版本。
                通过AsyncContext初始化网络事件发送能力。
                注册GET /ws到ws_handler。
                PRE_UPDATE安装abort_control_system。
                UPDATE安装dto_route_system及dto_sync_system。
```

# system

## 函数

crate公开：
```text
dto_route_system(world: &mut World)
    处理事件：DtoRequest、ClientDisconnected
    接收闭包结果：None
    产出事件：RegisterClient、StartWorkspace、StopWorkspace、RestartWorkspace、StartAgent、StopAgent、RestartAgent、MclCommand
    回复：各请求原oneshot
    行为：先处理断连，再逐条调用handle_dto_request。

abort_control_system(world: &mut World)
    处理事件：AbortTurnControl
    接收闭包结果：None
    产出事件：None
    回复：Result<(), ProtocolErrorDto>
    行为：调用handle_abort_control，在本次PRE_UPDATE校验目标及中止回合。

dto_sync_system(world: &mut World)
    处理事件：WebSocketMessageSend、AgentException
    接收闭包结果：Result<(), DtoAsyncError>
    产出事件：None
    回复：None
    提交闭包：run_ui_watch(UiMonitorContext)
    行为：处理出站及异常，记录worker错误，调用publish_state；调用reconcile_ui_watches维护实际Driver来源的观察，再调用publish_ui。
```

# handler

## 函数

crate公开：
```text
ws_handler(State(state): State<DtoState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Future<Output = Response>
    行为：评估入口策略，检查退出与事件能力，分配真实连接；取得跟踪guard并升级，完整连接退出后释放。

handle_dto_request(world: &mut World, request: DtoRequest)
    行为：取出object，按route选择具体转换类型或查询函数；转换后直接发送领域事件。agent.ui.req调用query_agent_ui，只读取当前UI观察结果。

handle_abort_control(world: &mut World, event: AbortTurnControl)
    行为：取出原请求，校验真实Client、Agent地址及turn；同步清turn，冻结并取消推理，裁决未return工具，通过原reply回复。

handle_disconnect(world: &mut World, connection: ConnectionId)
    行为：释放该连接的同步投影及UI发送版本；Client领域独立清理连接身份。Agent UI观察跟随Agent来源独立运行。

handle_agent_exception(world: &mut World, event: AgentException)
    行为：检查完整Entity并保存该Agent的故障投影。

handle_outbound(world: &World, event: WebSocketMessageSend)
    行为：按target查询实际ClientSender并发送已编码帧。

publish_state(world: &mut World)
    行为：
        每帧按配置查询并去重符合条件的ClientSender。
        读取Workspace、Agent、Turn、Task当前投影，变化时发送相应sync。
        按完整Agent Entity提交独立记录读取worker，各类别维护自己的真实sequence观察位置。
        数据库IO在blocking worker执行，World仅短锁取得进度。
        worker向本次发送器快照直接投递新增记录，拥塞连接被取消；失败释放在途标记以便后续重试。
        已结束Agent取消其记录读取能力并释放对应观察位置。
        req保持领域状态及同步接收条件不变。

reconcile_ui_watches(world: &mut World)
    行为：
        每帧查询Agent及AgentDriverSource，以完整Entity和实际manifest_path维护SyncState.ui。
        AxumLifetime已进入退出时取消已有watch并停止建立新watch。
        新来源建立DriverUiWatch及UiWatchSlot，on_change捕获该slot的delivery、地址及现有UPDATE的WorldClosureSender。
        初始delivery为AgentUiNotReady；通过普通异步闭包提交run_ui_watch，持有AxumWorkGuard直到完整监控退出。
        来源移除、替换或Agent消失时取消旧watch并移出索引；旧回调只持有旧slot，发送时仅使用当前索引中的slot。
        监控的创建和取消由该帧观察负责，查询只读取。

publish_ui(world: &mut World)
    行为：
        每帧按既定条件取得实际ClientSender，检查当前slot的Agent及来源仍匹配。
        读取UiDelivery的轻量key和已编码frame，与该连接该Agent上次成功发送key比较。
        新接收者及key变化时使用ClientSender发送ClientFrame::Text，成功入队后保存key；Full/Closed按连接规则取消。
        清理已离线连接、已移除Agent及来源替换前的发送版本。
        此函数只读取已编码资料并发送，资源扫描、base64及大JSON编码在监控worker内完成。

projection::address(world: &World, entity: Entity) -> Option<String>
    行为：读取Address组件。

projection::workspaces(world: &World) -> Vec<WorkspaceInfoDto>
    行为：合并已发布定义与运行组件；来源移除项使用Workspace.definition，WorkspaceFailure投影为error。

projection::agents(world: &World) -> Vec<AgentStateDto>
    行为：每次调用从Agent Entity读取Tools并遍历Sandboxes，读取SelectedSandbox；以AgentInferenceConfig的model关联ModelRoutes，投影模型ID、名称、provider、api_type、上下文窗口和思考选项，读取故障。缺失沙箱返回空数组，未选择返回null，配置或路由缺失时model为null。

projection::turn(world: &World, entity: Entity) -> Option<AgentTurnStateDto>
    行为：读取Agent地址与当前turn。

projection::tasks(world: &World, entity: Entity) -> Option<AgentTaskStateDto>
    行为：调用ToolTasks::snapshot取得完整窗口，保留脚本读取位置及replaced。
```

私有：
```text
serve_connection(state: DtoState, connection: ConnectionId, socket: WebSocket, pushes: mpsc::Receiver<ClientFrame>, cancel: CancellationToken) -> Future<Output = ()>
    行为：
        持有socket读写端及FuturesUnordered<ReplyFuture>。
        接收合法请求后将预处理及原接收端等待放入本连接并发集合。
        普通请求发送DtoRequest，中止请求直接发送AbortTurnControl。
        等待原oneshot结果后编码，socket写端统一发送响应、主动同步及协议控制帧。
        每连接最多128个在途请求，同一连接在途id唯一。
        退出关闭推送接收端、结束网络等待并由连接保留guard发送一次ClientDisconnected。

prepare_request(state: &DtoState, connection: ConnectionId, message: ClientMessage) -> Future<Output = Result<(Option<DtoRequest>, Option<AbortTurnControl>, ReplyFuture), Value>>
    行为：
        取得route及原id，为具体请求创建对应领域Result类型的原oneshot。
        发送端装入Prepared，原接收端与纯值投影函数组成ReplyFuture。
        单独Agent启动重启在worker执行全量reload，将本次不可变快照放入预处理对象。
        agent.ui.req创建原oneshot，结果为Result<Option<Arc<DriverUiBundle>>, ProtocolErrorDto>；网络等待结束后调用project_agent_ui并编码AgentUiRes。
        预处理Future独立推进，socket继续接收其他请求。

request<T, R, E>(ticket: Arc<Ticket>, route: &str, input: T, response: &str, project: impl FnOnce(R) -> Result<Value, ProtocolErrorDto> + Send + 'static) -> (DtoRequest, ReplyFuture)
    行为：创建原oneshot，保存具体输入与原发送端，接收端等待结束后映射领域错误及业务结果。

dispatch<T: IntoEvent>(world: &mut World, object: Box<dyn Any + Send>)
    行为：T::try_from取得具体边界类型，调用into_event，再world.send_event。

query<T, R>(world: &World, object: Box<dyn Any + Send>, read: impl FnOnce(&World, Entity, &T) -> Result<R, ProtocolErrorDto>)
    行为：校验Client后执行只读查询并通过原reply回复。

records(world: &World, route: &str, object: Box<dyn Any + Send>)
    行为：校验Agent，克隆AgentStorage；worker按RecordRange调用对应core分页函数，通过原reply回复。

reload(world: &World, object: Box<dyn Any + Send>)
    行为：校验Client，克隆WorkspaceDefinitions；worker全量reload并通过原reply回复。

query_agent_ui(world: &World, object: Box<dyn Any + Send>)
    行为：
        取出Prepared<AgentTargetDto, Option<Arc<DriverUiBundle>>>，校验真实Client及Agent地址。
        读取AgentDriverSource，核对SyncState.ui中的实际来源，再调用DriverUiWatch::state。
        Initializing或来源尚未进入观察时通过原reply返回AgentUiNotReady。
        Ready结果通过原reply返回；Failed映射DriverUi错误码；目标失效返回AgentMissing。
        不创建监控、不扫描文件、不改变发送关系。

wake_ui_frame(pending: Arc<AtomicBool>, world: &mut World)
    行为：由已安装的UPDATE ClosureSystem执行，清除轻量通知在途标记；同帧dto_sync_system读取最新delivery并筛选发送器。既有ClosureRequest入队本身保证事件驱动模式有帧工作。

project_agent_ui(value: Option<Arc<DriverUiBundle>>) -> Result<Option<AgentUiBundleDto>, ProtocolErrorDto>
    行为：将每个文件的原始字节编码为标准base64，保留规范相对路径、媒体类型、entry和revision。

check_ui_budget(address: &str, ui: &Option<AgentUiBundleDto>) -> Result<(), ProtocolErrorDto>
    行为：用最大可用请求id检查完整res信封预算，超出映射DriverUiTooLarge；网络查询和监控编码共同调用。

build_ui_delivery(address: &str, state: DriverUiState) -> UiDelivery
    行为：
        在监控worker内转换状态并构造AgentUiSync，资源正常时调用project_agent_ui。
        按sync及带最大请求id的res信封预算检查完整集合，超出转为DriverUiTooLarge错误状态。
        正常缺失UI编码ui=null/error=null，错误编码ui=null/error对象。
        保存轻量UiDeliveryKey及完整已编码frame，供帧循环读取；错误恢复也形成key变化。

run_ui_watch(context: UiMonitorContext) -> Future<Output = Result<(), DtoAsyncError>>
    行为：
        驱动agent_driver_core::run_driver_ui_monitor，on_change在blocking发布阶段调用build_ui_delivery，短锁替换delivery；使用frame_pending合并通知，通过WorldClosureSender::send提交wake_ui_frame。
        AxumLifetime退出时触发watch.cancel，继续等待monitor及正在执行的blocking读取真实返回。
        捕获监控异常并发布失败delivery后退出。完整Future退出后释放AxumWorkGuard，终结错误映射DtoAsyncError。

parameters(world: &World, input: &AgentInput) -> Result<(Entity, String, AgentStartParameters), ProtocolErrorDto>
    行为：按Workspace地址解析完整Entity，从本次快照选择定义及成员，构造单Agent启动参数。

client(world: &World, connection: ConnectionId) -> Result<Entity, ProtocolErrorDto>
target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
agent(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
    行为：查询真实连接身份、按地址定位完整Entity并校验目标组件。

deliver<R, E>(reply: &PreparedReply<R, E>, result: Result<R, E>)
reject<T, R, E: BoundaryError>(request: Prepared<T, R, E>, failure: ProtocolErrorDto)
    行为：认领原发送端一次并回复结果。

encode_response(value: Value) -> Option<String>
invalid_reply(text: &str) -> Option<Value>
frame(kind: &str, id: &str, result: Result<Value, ProtocolErrorDto>) -> Value
    行为：生成专属result或res，保持原id；过大响应返回预算错误，非法可关联请求返回专属失败。

mcl_result(value: MclValue) -> Result<Value, ProtocolErrorDto>
    行为：Unit确认成功，Error返回失败，其余值投影kind/value。

history_record(value: StoredHistoryRecord) -> Value
mcl_record(value: StoredHistoryRecord) -> Value
log_record(value: StoredAgentLog) -> Value
clean_internal(value: &mut Value)
    行为：投影规范持久记录及真实sequence，审计和日志中的VM标识留在后端。

recipients(world: &World, targets: &[OutboundTarget]) -> Vec<(ConnectionId, ClientSender)>
enqueue(world: &World, connection: ConnectionId, sender: &ClientSender, value: Value) -> bool
    行为：按实际Client组件筛选发送能力并有界发送，失败取消对应连接。
```

# types

## 类型

公开：
```text
WebSocketMessageSend
    target: OutboundTarget
    message: Value
    impl Event + Clone + Debug
```

私有：
```text
DtoState
    ingress: IngressPolicy
    events: Arc<OnceLock<AsyncContext>>
    connections: ConnState
    lifetime: AxumLifetime
    definitions: WorkspaceDefinitions
    impl Resource + Clone

Ticket
    id: String
    connection: ConnectionId
    active: Arc<Mutex<BTreeSet<String>>>
    impl Drop
        行为：释放本连接在途网络id。

PreparedReply<R, E> = Arc<Mutex<Option<oneshot::Sender<Result<R, E>>>>>

Prepared<T, R, E = ProtocolErrorDto>
    ticket: Arc<Ticket>
    input: T
    reply: PreparedReply<R, E>

DtoRequest
    route: String
    object: Arc<Mutex<Option<Box<dyn Any + Send>>>>
    impl Event + Clone

AbortTurnControl
    request: Arc<Mutex<Option<Prepared<AgentTurnAbortDto, ()>>>>
    impl Event + Clone

ReplyFuture = Pin<Box<dyn Future<Output = Value> + Send>>

IntoEvent
    type Event: core_plugin::Event
    impl TryFrom<Box<dyn Any + Send>, Error = Box<dyn Any + Send>>
    into_event(self, world: &World) -> Result<Self::Event, ()>

RegisterClientDto(Prepared<RegisterConnectionDto, RegisteredClient, ClientError>)
StartWorkspaceDto(Prepared<WorkspacePathDto, WorkspaceOperationOutput, WorkspaceError>)
StopWorkspaceDto(Prepared<WorkspacePathDto, WorkspaceOperationOutput, WorkspaceError>)
RestartWorkspaceDto(Prepared<WorkspacePathDto, WorkspaceOperationOutput, WorkspaceError>)
StartAgentDto(Prepared<AgentInput, Entity, AgentError>)
StopAgentDto(Prepared<AgentTargetDto, Entity, AgentError>)
RestartAgentDto(Prepared<RestartInput, Entity, AgentError>)
MclCommandDto(Prepared<MclExecuteDto, MclValue, MclError>)
    impl TryFrom<Box<dyn Any + Send>>
    impl IntoEvent
        行为：World校验及领域转换，原reply随事件传递；失败交付该请求自己的领域错误。

AgentInput
    start: AgentStartDto
    snapshot: WorkspaceDefinitionSnapshot

RestartInput
    target: AgentTargetDto
    snapshot: WorkspaceDefinitionSnapshot

BoundaryError
    boundary(value: ProtocolErrorDto) -> Self
    external(self) -> ProtocolErrorDto
    行为：实现于ProtocolErrorDto、ClientError、WorkspaceError、AgentError、MclError、StorageError。

ConnState
    entries: Arc<Mutex<BTreeMap<ConnectionId, ConnectionEntry>>>
    allocate(&self) -> Option<(ConnectionId, mpsc::Receiver<ClientFrame>, CancellationToken)>
    get(&self, id: ConnectionId) -> Option<ClientSender>
    claim(&self, id: ConnectionId, request: &str) -> Option<Arc<Ticket>>
    cancel(&self, id: ConnectionId)
    remove(&self, id: ConnectionId) -> bool
    impl Resource + Clone + Default

ConnectionEntry
    sender: ClientSender
    cancel: CancellationToken
    active: Arc<Mutex<BTreeSet<String>>>

SyncState
    sent: BTreeMap<(u64, String), Value>
    errors: BTreeMap<Entity, ProtocolErrorDto>
    records: BTreeMap<Entity, Arc<Mutex<RecordPosition>>>
    ui: BTreeMap<Entity, UiWatchSlot>
    ui_sent: BTreeMap<(ConnectionId, Entity), UiDeliveryKey>
    impl Resource + Default

UiWatchSlot
    manifest_path: PathBuf
    address: String
    watch: DriverUiWatch
    delivery: Arc<RwLock<UiDelivery>>
    frame_pending: Arc<AtomicBool>
UiMonitorContext
    monitor: DriverUiMonitor
    watch: DriverUiWatch
    lifetime: AxumLifetime
    guard: AxumWorkGuard
UiDelivery
    key: UiDeliveryKey
    frame: Arc<str>
UiDeliveryKey
    Initializing
    Missing
    Ready(String)
    Failed { code: String, message: String }
    impl Clone + PartialEq + Eq

RecordPosition
    cancel: CancellationToken
    initialized: bool
    running: bool
    message: u64
    mcl: u64
    log: u64

DtoInitialized
DtoAsyncError(String)
    impl From<AsyncTaskError>
```

# 持有关系

```text
World
├── DtoState
├── ConnState
│   └── ConnectionEntry
│       ├── ClientSender
│       ├── CancellationToken
│       └── 在途id集合
└── SyncState
    ├── 分类已发送投影
    ├── Agent故障投影
    ├── 记录观察进度
    └── 每Agent的UiWatchSlot及每连接发送版本
        ├── DriverUiWatch
        └── 已编码UiDelivery

serve_connection
├── WebSocket读写端
├── 推送mpsc接收端
└── FuturesUnordered<ReplyFuture>
    ├── Ticket
    └── 原oneshot接收端

DtoRequest
└── 具体Prepared对象
    ├── 协议输入
    ├── Ticket
    └── 原oneshot发送端
        └── 领域事件
            └── 领域执行结果
```
