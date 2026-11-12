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
# handler  dto/plugin/src/handler.rs、projection.rs、ui.rs、records.rs
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
                初始化DtoState、ConnState及SyncState；SyncState包含按Agent持有的记录监控、UI观察及发送版本。
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
    监控产出事件：仅在实际更新时发送WebSocketMessageSend
    回复：None
    提交闭包：仅为新出现的Agent存储或Driver来源启动一次run_record_watch或run_ui_watch
    行为：
        WebSocketMessageSend调用handle_outbound，按当次World筛选ClientSender并发送，该分支终结本次出站事件。
        AgentException调用handle_agent_exception，更新故障投影。
        监控退出结果用于诊断和生命周期收口；退出本身不触发重新启动。
        每个活动帧调用publish_state读取组件投影，再调用reconcile_record_watches及reconcile_ui_watches维护监控归属，最后publish_ui处理当前接收者的UI版本。
        组件内容不变且监控来源不变时，该帧不提交调查闭包、不产生事件。
        异步监控只有新增持久记录、UI内容变化或错误状态变化时产出WebSocketMessageSend。

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

handle_outbound(world: &mut World, event: WebSocketMessageSend)
    行为：
        按event.target及当次出站配置查询实际Client组件，按ConnectionId去重，取得仍存活的ClientSender；UI事件调用publish_ui发送当前索引的最新delivery并按版本去重。
        监控发送target=Broadcast；接收端以当前backend_state条件限制同步接收范围。
        UI消息以当前slot缓存为准，迟到旧通知不会把旧资源内容发给接收者。
        编码一次并向符合条件的发送能力有界投递，Full/Closed按既有规则取消连接。
        处理结果不再发送WebSocketMessageSend或调查请求；UI发送成功记录对应版本；记录监控位置只在有实际更新放入事件通道后推进。

publish_state(world: &mut World)
    行为：
        每个活动帧按配置查询并去重符合条件的ClientSender。
        只读取Workspace、Agent、Turn、Task组件及领域已有投影，比较上次成功入队值。
        新接收者或内容有变化时直接发送对应sync并保存交付投影，内容相同不重复发送。
        本函数不读取数据库，不创建后台调查任务，不发事件或唤醒帧。
        req保持领域状态及同步接收条件不变。

reconcile_record_watches(world: &mut World)
    行为：
        以完整Agent Entity及实际AgentStorage来源维护SyncState.records。
        首次出现该存储时创建一个RecordWatchSlot，克隆存储能力、事件发送能力和独立取消能力，提交一次run_record_watch。
        后续活动帧只核对归属；相同来源沿用原监控。
        Agent停止、存储替换或AxumLifetime退出时取消原监控，等待真实读取任务收口；新来源独立建立监控。
        异常退出不在每个活动帧自动重新创建任务，读取恢复由监控内部错误状态及250ms退避逻辑处理。

reconcile_ui_watches(world: &mut World)
    行为：
        每个活动帧查询Agent及AgentDriverSource，以完整Entity和实际manifest_path维护SyncState.ui。
        AxumLifetime已进入退出时取消已有watch并停止建立新watch。
        新来源建立DriverUiWatch及UiWatchSlot，on_change捕获该slot的delivery、地址和RuntimeEventSender。
        初始delivery为AgentUiNotReady，通过普通异步闭包仅提交一次run_ui_watch，持有AxumWorkGuard直到完整监控退出。
        来源移除、替换或Agent消失时取消旧watch并移出索引；旧回调更新旧slot，World出站分支核对当前归属。
        监控创建由新来源条件触发，查询只读取。

publish_ui(world: &mut World)
    行为：
        每个活动帧按既定条件取得实际ClientSender，检查当前slot的Agent及来源仍匹配。
        读取UiDelivery的轻量key和已编码frame，与该连接该Agent上次成功发送key比较。
        新接收者或key变化时直接有界投递ClientFrame::Text，成功后保存key；Full/Closed按连接规则取消。
        清理已离线连接、已移除Agent及来源替换前的发送版本。
        本函数只读缓存并发送；不提交调查闭包、不发事件、不唤醒帧。

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

project_agent_ui(value: Option<Arc<DriverUiBundle>>) -> Result<Option<AgentUiBundleDto>, ProtocolErrorDto>
    行为：将每个文件的原始字节编码为标准base64，保留规范相对路径、媒体类型、entry和revision。

check_ui_budget(address: &str, ui: &Option<AgentUiBundleDto>) -> Result<(), ProtocolErrorDto>
    行为：用最大可用请求id检查完整res信封预算，超出映射DriverUiTooLarge；网络查询和监控编码共同调用。

build_ui_delivery(address: &str, state: DriverUiState) -> UiDelivery
    行为：
        在监控worker内转换状态并构造AgentUiSync，资源正常时调用project_agent_ui。
        按sync及带最大请求id的res信封预算检查完整集合，超出转为DriverUiTooLarge错误状态。
        正常缺失UI编码ui=null/error=null，错误编码ui=null/error对象。
        保存轻量UiDeliveryKey及完整已编码frame，供活动帧读取；错误恢复也形成key变化。

read_batch(storage: &AgentStorage, position: RecordWatermarks, high: RecordWatermarks) -> Result<RecordBatch, StorageError>
    行为：仅查询高水位比position大的表，每表最多50条，排他上界为本次high加1；最大SQLite整数边界使用无上界读取。

encode_batch(address: &str, batch: &RecordBatch) -> Result<Vec<Value>, DtoAsyncError>
    行为：投影非空消息full、MCL及日志增量，检查实际协议编码及预算，只为实际记录生成消息值。

run_record_watch(context: RecordMonitorContext) -> Future<Output = Result<(), DtoAsyncError>>
    行为：
        reconcile_record_watches创建时先调用storage_core::subscribe_record_changes，读取watch的已提交高水位作为初始position并标记该版本已见；异步监控启动间隙的新提交仍保留changed通知。
        无通知时异步等待changes.changed、取消或生命周期退出，不提交SQL、不创建调查任务、不发事件。
        收到真实提交版本变化后取watch当次高水位，仅对有新增sequence的类别在read_batch执行索引范围查询。
        分页读取到本次高水位所覆盖的记录，页内数据投影完成后，非空结果发送WebSocketMessageSend；空页不发送出站事件。
        每批成功放入事件通道后推进监控位置，继续读未完成的同次变化；全部追齐后回到changes.changed等待。
        读取期间发生新提交时保留通知，当前批次之后继续追齐，合并版本通知不丢记录。
        读取失败保存原位置并有间隔重试，相同错误只记录一次；错误没有新状态时不制造完成事件或新帧。
        每个监控同时最多一个blocking读取；任务完成只回到本监控，不产生每批Result完成事件。
        取消后等待真实文件worker结束，完整监控退出后释放AxumWorkGuard；结束结果只交付一次。

run_ui_watch(context: UiMonitorContext) -> Future<Output = Result<(), DtoAsyncError>>
    行为：
        驱动agent_driver_core::run_driver_ui_monitor，稳定初始状态及后续真实变化在worker中调用build_ui_delivery。
        短锁更新当前delivery，仅当UiDeliveryKey变化时从已编码frame取得完整同步Value并发送WebSocketMessageSend，target=Broadcast。
        相同内容、相同错误或无文件变化不发事件；内容恢复及错误恢复属于真实状态变化。
        异步任务只持owned事件发送能力和观察状态，不访问World或持有ClientSender快照。
        AxumLifetime退出时触发watch.cancel，继续等待monitor及正在执行的blocking读取真实返回。
        捕获监控异常并发布一次失败更新后退出，完整Future退出后释放AxumWorkGuard，终结错误映射DtoAsyncError。

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
    行为：异步监控产生实际网络更新后投递；System收到后查询当次ClientSender并发送，保持事件字段不变。
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
    records: BTreeMap<Entity, RecordWatchSlot>
    ui: BTreeMap<Entity, UiWatchSlot>
    ui_sent: BTreeMap<(ConnectionId, Entity), UiDeliveryKey>
    impl Resource + Default

UiWatchSlot
    manifest_path: PathBuf
    address: String
    watch: DriverUiWatch
    delivery: Arc<RwLock<UiDelivery>>
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

RecordWatchSlot
    storage: AgentStorage
    cancel: CancellationToken
    impl Drop
        行为：取消该来源的持续监控。
RecordBatch
    messages: Vec<StoredHistoryRecord>
    mcl: Vec<StoredHistoryRecord>
    logs: Vec<StoredAgentLog>
RecordMonitorContext
    changes: RecordChanges
    position: RecordWatermarks
    storage: AgentStorage
    address: String
    events: RuntimeEventSender
    cancel: CancellationToken
    lifetime: AxumLifetime
    guard: AxumWorkGuard
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
    ├── 每Agent持续记录监控及取消能力
    └── 每Agent的UiWatchSlot及每连接发送版本
        ├── DriverUiWatch
        └── 已编码UiDelivery

异步监控
├── 真实提交通知或文件变化
├── owned读取任务
└── 有实际更新时发送WebSocketMessageSend
    └── dto_sync_system
        └── 当次ClientSender

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
