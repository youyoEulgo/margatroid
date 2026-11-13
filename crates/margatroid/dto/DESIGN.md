# 伪代码格式

```text
板块使用一级标题，类型及函数使用二级标题。
类型列出实际Rust字段和持有关系，函数列出签名及直接行为。
System列出处理事件、闭包结果、产出事件及回复；实施状态见README。
```

# 板块格式

```text
# lib      dto/plugin/src/lib.rs
# system   dto/plugin/src/system.rs
# handler  dto/plugin/src/handler.rs、projection.rs、ui.rs、records.rs、probes.rs、workspaces.rs、monitor.rs
# types    dto/plugin/src/types.rs
```

# lib

## 类型

公开：
```text
DtoPlugin：无字段，Default
    new() -> Self
    impl Plugin
        build(self, app: &mut App)
            行为：检查Runtime/AsyncRuntime/Axum及MargatroidConfig，复用World唯一Clients；存在的typed绑定必须与该槽位一致。
                取得或创建应用级WorkspaceSyncBinding及AgentUpdateContext，安装资源probe工厂，初始化DtoState及只持监控归属的SyncObservations。
                为安装前已有Agent初始化实际来源观察，在应用启动建立一次Workspace定义磁盘监控。
                注册GET /ws；只安装PRE_UPDATE的abort_control_system和UPDATE的dto_route_system，不安装同步System。
task_sync_binding(clients: Clients, targets: &[OutboundTarget]) -> TaskSyncBinding
    行为：构造Task完整快照的DTO编码函数和同一共享槽位，注入ToolPlugin。
agent_sync_context(clients: Clients, targets: &[OutboundTarget]) -> AgentUpdateContext
    行为：创建一个应用级AgentViews、完整Agent集合来源及per-Agent turn来源；World同步入口提交owned投影，公共资源回调提交typed派生字段。
resource_sync_bindings(context: AgentUpdateContext) -> (ToolResourceSyncBinding, SandboxSyncBinding, SandboxSelectionSyncBinding)
model_sync_binding(context: AgentUpdateContext) -> ModelSyncBinding
    行为：按owner衍生实际来源scope，映射安全Tools/沙箱/模型纯值；旧scope失效后回调不生效，不跨plugin依赖。
workspace_sync_binding(clients: Clients, targets: &[OutboundTarget]) -> WorkspaceSyncBinding
    行为：创建应用级WorkspaceViews，定义成功/错误观察和生命周期探针共同更新同一owned列表。
sync_selectors(targets: &[OutboundTarget]) -> Arc<[ClientSelector]>
    行为：明确转换Broadcast/Type/Name，不从任意wire消息猜投递范围。

```

# system

## 函数

crate公开：
```text
dto_route_system(world: &mut World)
    处理事件：DtoRequest
    接收闭包结果：None
    产出事件：RegisterClient、StartWorkspace、StopWorkspace、RestartWorkspace、StartAgent、StopAgent、RestartAgent、MclCommand
    回复：各请求原oneshot
    行为：逐条受理普通请求，业务更新由所属领域统一入口直接同步，不在此轮询投影。

abort_control_system(world: &mut World)
    处理事件：AbortTurnControl
    接收闭包结果：None
    产出事件：None
    回复：Result<(), ProtocolErrorDto>
    行为：调用handle_abort_control，经Agent统一入口同步清turn，再执行推理冻结及工具中止裁决；各领域入口直接发布实际变化。
```

# handler

## 函数

crate公开：
```text
ws_handler(State(state): State<DtoState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Future<Output = Response>
    行为：评估入口策略和退出状态，通过state.clients.allocate取得真实连接、唯一ClientReceiver及取消能力；升级任务持有AxumWorkGuard。

handle_dto_request(world: &mut World, request: DtoRequest)
    行为：取出具体owned对象，按route转换领域请求或只读查询；原reply继续沿领域执行链回复。

handle_abort_control(world: &mut World, event: AbortTurnControl)
    行为：认领原请求，校验真实Client、Agent及turn，调用WorldAgentExt的统一turn入口及相关领域取消能力；原oneshot回复。

projection::address(world: &World, entity: Entity) -> Option<String>
projection::workspaces(world: &World) -> Vec<WorkspaceInfoDto>
projection::agents(world: &World) -> Vec<AgentStateDto>
projection::turn(world: &World, entity: Entity) -> Option<AgentTurnStateDto>
projection::tasks(world: &World, entity: Entity) -> Option<AgentTaskStateDto>
    行为：仅服务req的非破坏性只读查询，从所属领域共享状态取得完整快照；不创建观察、不推进脚本cursor、不触发sync或IO调查。

state_source<T>(clients: Clients, targets: Arc<[ClientSelector]>, key: SyncKey, view: &View<T>, encode: impl Fn(T) -> Result<Arc<ClientFrame>, ClientPrepareError> + Send + Sync + 'static) -> SyncContext<T>
    行为：prepare只捕获view的Weak，升级后在短锁读取同版本owned快照，实际读取和编码在ClientReceiver监督的blocking worker执行。
update<T>(view: &View<T>, context: &SyncContext<T>, value: T)
    行为：短锁比较实际值，变化才递增版本，释放锁后publish；不产生事件或World帧。
initialize_agent_sources(world: &mut World)
    行为：DTO首次安装时处理已有Agent，之后只由实际生命周期notify创建/关闭观察，不重复按帧扫描。
get_or_create_agent_context(world: &mut World) -> AgentUpdateContext
install_resource_bindings(world: &mut World, context: AgentUpdateContext)
    行为：复用装配层现存能力，独立DTO测试未安装时从同一Clients构造并插入typed工厂。
refresh_ui_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
refresh_record_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
    行为：只在实际来源安装回调核对对应实体；同来源无操作，旧来源取消并返回真实完成等待，新来源建立一次owned monitor并持有AxumWorkGuard。
stop_ui_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
stop_record_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
    行为：移除并失效对应source、请求取消，返回Completion等待；Agent生命周期真实await之后才能宣告资源释放。
run_ui_watch(context: UiMonitorContext) -> Future<Output = Result<(), DtoAsyncError>>
    行为：驱动现有Driver文件观察；稳定变化更新版本化UiDelivery并经SyncContext直投StateKey，重复内容/错误不发布；取消仍等待实际文件worker。
run_record_watch(context: RecordMonitorContext) -> Future<Output = Result<(), DtoAsyncError>>
    行为：等待实际COMMIT的watch高水位，只对新增sequence执行有界索引查询；encode_batch生成具体有序帧，Clients.send_scoped直接选择当前接收者，追齐后等待，不产出ECS同步事件。
read_batch(storage: &AgentStorage, position: RecordWatermarks, high: RecordWatermarks) -> Result<RecordBatch, StorageError>
encode_batch(address: &str, batch: &RecordBatch) -> Result<Vec<Arc<ClientFrame>>, DtoAsyncError>
    行为：每类最多50条，当前高水位限定排他范围；消息full保留真实持久序号，MCL和日志过滤内部VM信息，完整编码/预算失败明确重试诊断而不重复旧记录。
install_workspace_monitor(world: &mut World)
    行为：安装前已有定义和运行状态先发布，绑定同一WorkspaceDefinitions.observe回调；实际配置来源存在时建立一次watch_definitions，归属AxumLifetime。
watch_definitions(source: PathBuf, definitions: WorkspaceDefinitions, lifetime: AxumLifetime) -> Future<Output = Result<(), String>>
    行为：notify观察来源父目录及子树，过滤Access/无关路径；先挂观察再初读，100ms安静窗口合并；同一时间只一个blocking reload，完整校验后由定义入口observe直接发布。
        错误保留最后有效定义并记录现有错误状态，恢复清故障；取消等待实际reload返回，完整Future释放guard，不发同步事件或唤醒帧。

```

私有：
```text
serve_connection(state: DtoState, connection: ConnectionId, socket: WebSocket, pushes: ClientReceiver, cancel: CancellationToken) -> Future<Output = ()>
    行为：
        持有唯一socket读写端和FuturesUnordered<ReplyFuture>，等待普通请求、原oneshot结果、唯一出站队列及退出。
        Ordered帧先检查可选来源active，再保持入队顺序发送；正常生成结束的accepting=false不抹除已入队full；StateKey从同一ClientReceiver取最新pending来源，失效来源直接跳过。
        来源prepare持有可独立读取的Arc状态，必要IO/编码在受监督blocking worker完成，不访问World。
        每连接至多准备一个状态快照，准备期间继续接收请求、控制帧及取消；不每次更新创建无界worker。
        完成快照后核验source epoch、revision及协议预算；旧来源或已发送版本跳过，实际发送由唯一writer串行进行。
        状态更新在准备期间可合并为一个新的StateKey；有序chunk及记录不作此合并。
        入队Full/Closed按ClientSender规则取消慢连接；编码/预算错误明确诊断并关闭受影响连接，不静默重试。
        退出先clients.remove，关闭队列、清空pending和请求等待，等待本连接真实准备worker结束，再发送一次ClientDisconnected并释放guard。

prepare_request(state: &DtoState, connection: ConnectionId, message: ClientMessage) -> Future<Output = Result<(Option<DtoRequest>, Option<AbortTurnControl>, ReplyFuture), Value>>
    行为：
        从唯一Clients.claim取得ticket，创建具体领域Result的原oneshot。
        普通请求包装DtoRequest，中止包装AbortTurnControl；注册只携带ConnectionId及自报标签，不复制sender或网络注册表。
        注册成功后Client领域已经原子发布身份；一次性回复不绕回DTO完成事件。
        单独Agent启动/重启仍在worker全量reload并把本次不可变快照放入具体预处理对象。
        UI查询沿原oneshot返回观察状态；base64和wire编码保持在网络边界。

request<T, R, E>(ticket: Arc<RequestTicket>, route: &str, input: T, response: &str, project: impl FnOnce(R) -> Result<Value, ProtocolErrorDto> + Send + 'static) -> (DtoRequest, ReplyFuture)
    行为：原接收端与纯投影组成ReplyFuture，原发送端交给具体领域。
dispatch<T: IntoEvent>(world: &mut World, object: Box<dyn Any + Send>)
    行为：具体TryFrom后发送领域请求事件；这是业务请求受理，不承担sync中转。
query<T, R>(world: &World, object: Box<dyn Any + Send>, read: impl FnOnce(&World, Entity, &T) -> Result<R, ProtocolErrorDto>)
    行为：校验真实Client及目标后只读并完成原reply。
records(world: &World, route: &str, object: Box<dyn Any + Send>)
    行为：克隆实际AgentStorage，由worker调用对应分页core函数，通过原reply返回，不建立监控。
reload(world: &World, object: Box<dyn Any + Send>)
    行为：校验Client，克隆WorkspaceDefinitions，执行同一全量reload并回复；成功发布也经Workspace统一入口同步。
query_agent_ui(world: &World, object: Box<dyn Any + Send>)
    行为：核对当前Agent及实际Driver来源，读取该来源已有观察状态；未就绪返回AgentUiNotReady，不创建监控。

project_agent_ui(value: Option<Arc<DriverUiBundle>>) -> Result<Option<AgentUiBundleDto>, ProtocolErrorDto>
    行为：保持原字节、排序相对路径、媒体类型、entry及内容revision，data使用标准base64。
check_ui_budget(address: &str, ui: &Option<AgentUiBundleDto>) -> Result<(), ProtocolErrorDto>
    行为：同时检查完整sync及最大合法id的res信封，超出映射DriverUiTooLarge。
build_ui_delivery(address: &str, state: DriverUiState) -> UiDelivery
    行为：投影完整稳定UI集合或错误并编码一次，内容key去重；内部单调版本负责时序，摘要revision不作时间排序。
parameters(world: &World, input: &AgentInput) -> Result<(Entity, String, AgentStartParameters), ProtocolErrorDto>
client(world: &World, connection: ConnectionId) -> Result<Entity, ProtocolErrorDto>
target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
agent(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
    行为：使用实际连接身份与Address定位完整Entity；Agent启动参数仍取同次全量reload快照。
deliver<R, E>(reply: &PreparedReply<R, E>, result: Result<R, E>)
reject<T, R, E: BoundaryError>(request: Prepared<T, R, E>, failure: ProtocolErrorDto)
    行为：只认领原oneshot发送端一次。
encode_response(value: Value) -> Option<String>
invalid_reply(text: &str) -> Option<Value>
frame(kind: &str, id: &str, result: Result<Value, ProtocolErrorDto>) -> Value
mcl_result(value: MclValue) -> Result<Value, ProtocolErrorDto>
history_record(value: StoredHistoryRecord) -> Value
mcl_record(value: StoredHistoryRecord) -> Value
log_record(value: StoredAgentLog) -> Value
clean_internal(value: &mut Value)
    行为：保持原网络type/id、规范持久内容及真实sequence，内部VM元数据仅在网络边界过滤。
```

# types

## 类型

公开：
```text
WorkspaceSyncBinding
    updates: WorkspaceUpdateContext
    views: Arc<WorkspaceViews>
    impl Resource + Clone
```

私有：
```text
DtoState
    ingress: IngressPolicy
    events: Arc<OnceLock<AsyncContext>>--只用于业务请求和连接生命周期
    clients: Clients
    lifetime: AxumLifetime
    definitions: WorkspaceDefinitions
    impl Resource + Clone
Ticket = RequestTicket
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
    into_event(self, world: &World) -> Result<Self::Event, ()>
RegisterClientDto(Prepared<RegisterConnectionDto, RegisteredClient, ClientError>)
StartWorkspaceDto(Prepared<WorkspacePathDto, WorkspaceOperationOutput, WorkspaceError>)
StopWorkspaceDto(Prepared<WorkspacePathDto, WorkspaceOperationOutput, WorkspaceError>)
RestartWorkspaceDto(Prepared<WorkspacePathDto, WorkspaceOperationOutput, WorkspaceError>)
StartAgentDto(Prepared<AgentInput, Entity, AgentError>)
StopAgentDto(Prepared<AgentTargetDto, Entity, AgentError>)
RestartAgentDto(Prepared<RestartInput, Entity, AgentError>)
MclCommandDto(Prepared<MclExecuteDto, MclValue, MclError>)
    impl TryFrom<Box<dyn Any + Send>> + IntoEvent
AgentInput
    start: AgentStartDto
    snapshot: WorkspaceDefinitionSnapshot
RestartInput
    target: AgentTargetDto
    snapshot: WorkspaceDefinitionSnapshot
BoundaryError
    boundary(value: ProtocolErrorDto) -> Self
    external(self) -> ProtocolErrorDto
SyncObservations：Resource
    records: BTreeMap<Entity, RecordWatchSlot>
    ui: BTreeMap<Entity, UiWatchSlot>
    行为：只持监控归属和查询能力，不保存每连接全量投影或每帧发送位置。
UiWatchSlot
    completion: Arc<MonitorCompletion>
    manifest_path: PathBuf
    address: String
    watch: DriverUiWatch
    sync: SyncContext<UiDelivery>
    impl Drop：失效source并取消watch。
RecordWatchSlot
    completion: Arc<MonitorCompletion>
    storage: AgentStorage
    cancel: CancellationToken
    active: Arc<AtomicBool>
    impl Drop：active=false并取消monitor。
UiMonitorContext
    completion: MonitorCompletionGuard
    monitor: DriverUiMonitor
    watch: DriverUiWatch
    lifetime: AxumLifetime
    guard: AxumWorkGuard
RecordMonitorContext
    completion: MonitorCompletionGuard
    storage: AgentStorage
    address: String
    clients: Clients
    targets: Arc<[ClientSelector]>
    active: Arc<AtomicBool>
    changes: RecordChanges
    position: RecordWatermarks
    cancel: CancellationToken
    lifetime: AxumLifetime
    guard: AxumWorkGuard
RecordBatch
    messages: Vec<StoredHistoryRecord>
    mcl: Vec<StoredHistoryRecord>
    logs: Vec<StoredAgentLog>
UiDelivery
    key: UiDeliveryKey
    frame: Arc<str>
UiDeliveryKey
    Initializing | Missing | Ready(String) | Failed { code: String, message: String }
View<T> = Arc<Mutex<(u64, T)>>
AgentViews
    clients: Clients
    targets: Arc<[ClientSelector]>
    view: View<Vec<AgentStateDto>>
    sync: SyncContext<Vec<AgentStateDto>>
    turns: Mutex<BTreeMap<Entity, TurnView>>
    addresses: Mutex<BTreeMap<Entity, String>>
TurnView
    view: View<AgentTurnStateDto>
    sync: SyncContext<AgentTurnStateDto>
WorkspaceViews
    catalog: Mutex<Catalog>
    view: View<Vec<WorkspaceInfoDto>>
    sync: SyncContext<Vec<WorkspaceInfoDto>>
Catalog
    definitions: BTreeMap<PathBuf, WorkspaceDefinition>
    running: BTreeMap<PathBuf, LiveWorkspace>
    definition_error: Option<String>
LiveWorkspace
    definition: Option<WorkspaceDefinition>
    status: String
    address: Option<String>
    error: Option<ProtocolErrorDto>
    行为：定义读取和运行投影的最终列表/版本提交受同一catalog锁串行，避免旧owned结果晚提交回滚状态。
MonitorCompletion
    done: AtomicBool
    changed: Notify
    wait(self: Arc<Self>) -> AgentObservationWait
MonitorCompletionGuard(Arc<MonitorCompletion>)
    impl Drop：真实monitor Future结束才设done并通知等待者。
DtoInitialized
```

# 持有关系

```text
compose
├── 唯一Clients Arc槽位
├── DTO具体编码函数及typed probe工厂
│   ├── Workspace定义磁盘监控及运行信息统一入口
│   ├── Agent/Turn/Tools/Sandbox/Model公共信息统一入口
│   ├── Task创建、输出、Return、错误、移除统一入口
│   ├── Driver UI磁盘监控
│   ├── 成功提交驱动的持久记录监控
│   └── 推理chunk/full执行链
└── 所有发送源直接遍历Clients并投递ClientSender
    └── 每连接唯一有界出站队列 → Axum唯一writer → WS

World DTO
├── dto_route_system：业务请求
└── abort_control_system：中止控制

已删除：dto_sync_system、WebSocketMessageSend、ConnState副本、每帧同步调查、sync中转通道。
```
