# 伪代码格式

```text
板块使用一级标题，类型及函数使用二级标题。
类型列出目标Rust字段和持有关系，函数列出签名及直接行为。
System列出处理事件、闭包结果、产出事件及回复；目标实施状态见README。
```

# 板块格式

```text
# lib      dto/plugin/src/lib.rs
# system   dto/plugin/src/system.rs
# handler  dto/plugin/src/handler.rs、projection.rs、ui.rs、records.rs、sync.rs
# types    dto/plugin/src/types.rs
```

# lib

## 类型

公开：
```text
DtoPlugin
    clients: Clients
    bindings: SyncBindings
    new(clients: Clients, bindings: SyncBindings) -> Self
    impl Plugin
        build(self, app: &mut App)
            行为：
                检查Runtime、AsyncRuntime、Axum及MargatroidConfig，核验clients与World的Clients为同一个Arc。
                将同一Clients及已装配的领域同步上下文放入DtoState，原生注册GET /ws。
                PRE_UPDATE安装abort_control_system，UPDATE只安装dto_route_system。
                不安装dto_sync_system，不创建sync中转通道或统一异步分发器。

create_sync_bindings(clients: Clients, config: &MargatroidConfig) -> SyncBindings
    行为：
        由compose在领域插件安装前调用，转换ClientSelector并构造具体领域的快照读取和编码回调。
        每个SyncContext/StreamContext持有同一Clients槽位；不复制当前接收者列表。
        workspace.sync与agent.sync各自只注册一个应用级集合来源；各实例更新上下文克隆同一集合发布能力，不按Agent反复替换集合source。
        turn/task/ui按Agent实际来源分别注册，记录及推理使用不进入状态索引的有序来源租约。
        具体wire投影留在DTO，领域core只使用自己的纯值类型及client_core的发送能力。
        显式构造Workspace、Agent、Turn、Task、UI及持久记录/推理流绑定，不用任意JSON反向查询World。
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
    行为：断连仅做请求归属清理；逐条受理普通请求，业务更新由所属领域统一入口直接同步，不在此轮询投影。

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

handle_disconnect(world: &mut World, connection: ConnectionId)
    行为：释放该连接在途请求归属；共享槽位已由传输退出路径移除，领域Client由ClientDisconnected幂等销毁。

projection::address(world: &World, entity: Entity) -> Option<String>
projection::workspaces(world: &World) -> Vec<WorkspaceInfoDto>
projection::agents(world: &World) -> Vec<AgentStateDto>
projection::turn(world: &World, entity: Entity) -> Option<AgentTurnStateDto>
projection::tasks(world: &World, entity: Entity) -> Option<AgentTaskStateDto>
    行为：仅服务req的非破坏性只读查询，从所属领域共享状态取得完整快照；不创建观察、不推进脚本cursor、不触发sync或IO调查。

encode_workspace_sync(snapshot: WorkspaceSyncSnapshot) -> Result<Arc<ClientFrame>, ClientPrepareError>
    行为：合并最新有效定义与运行实例的自有定义，投影完整workspace.sync列表。
encode_agent_sync(snapshot: AgentSyncSnapshot) -> Result<Arc<ClientFrame>, ClientPrepareError>
    行为：投影完整agent.sync列表，模型仅含安全字段，Tools/Sandboxes来自统一更新维护的公共视图。
encode_turn_sync(snapshot: AgentTurnSnapshot) -> Result<Arc<ClientFrame>, ClientPrepareError>
    行为：生成address及可空turn，后端来源epoch不进入wire。
encode_task_sync(snapshot: AgentTaskSnapshot) -> Result<Arc<ClientFrame>, ClientPrepareError>
    行为：生成完整活跃任务集合及完整保留窗口，清理后的集合不包含已结束项，不消费read游标。
encode_record_update(update: StorageRecordUpdate) -> Result<Arc<ClientFrame>, ClientPrepareError>
    行为：按真实类别生成新增message.full、agent.mcl或agent.log帧，保留真实sequence并过滤内部VM元数据；不合并或丢弃独立记录。
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
    行为：由UI观察回调生成完整已编码集合或错误，保留最新一次状态；只有内容或错误实际变化才通知对应SyncContext。

parameters(world: &World, input: &AgentInput) -> Result<(Entity, String, AgentStartParameters), ProtocolErrorDto>
client(world: &World, connection: ConnectionId) -> Result<Entity, ProtocolErrorDto>
target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
agent(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
    行为：按实际连接身份和Address定位完整Entity，Agent参数仍取本次reload快照。
deliver<R, E>(reply: &PreparedReply<R, E>, result: Result<R, E>)
reject<T, R, E: BoundaryError>(request: Prepared<T, R, E>, failure: ProtocolErrorDto)
    行为：只认领原发送端一次。
encode_response(value: Value) -> Option<String>
invalid_reply(text: &str) -> Option<Value>
frame(kind: &str, id: &str, result: Result<Value, ProtocolErrorDto>) -> Value
mcl_result(value: MclValue) -> Result<Value, ProtocolErrorDto>
    行为：保持原专属type/id、错误及MCL kind/value投影，过大res明确失败。
history_record(value: StoredHistoryRecord) -> Value
mcl_record(value: StoredHistoryRecord) -> Value
log_record(value: StoredAgentLog) -> Value
clean_internal(value: &mut Value)
    行为：保持规范持久内容及真实sequence，内部VM元数据仅在网络边界过滤。
```

# types

## 类型

公开：
```text
SyncBindings：compose创建并注入领域插件与DTO的具体同步绑定
    clients: Clients
    workspace: WorkspaceSyncBinding
    agent: AgentSyncBinding
    tasks: TaskSyncBinding
    ui: DriverUiSyncBinding
    records: StorageSyncBinding
    inference: InferenceSyncBinding
    行为：各binding只包含所属领域core的上下文工厂、typed快照/编码函数和同一Client槽位；不新建Client表，不提供通用命令或sync总线。
```

私有：
```text
DtoState
    ingress: IngressPolicy
    events: Arc<OnceLock<AsyncContext>>--仅普通请求及连接生命周期使用
    clients: Clients
    bindings: SyncBindings
    lifetime: AxumLifetime
    definitions: WorkspaceDefinitions
    impl Resource + Clone
PreparedReply<R, E> = Arc<Mutex<Option<oneshot::Sender<Result<R, E>>>>>
Prepared<T, R, E = ProtocolErrorDto>
    ticket: Arc<RequestTicket>
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
UiDelivery
    key: UiDeliveryKey
    frame: Arc<str>
UiDeliveryKey
    Initializing | Missing | Ready(String) | Failed { code: String, message: String }
    行为：内容revision只作内容身份，不比较其时间先后；内部source epoch与序号负责时序。
DtoInitialized
```

# 持有关系

```text
compose
├── 唯一Clients Arc槽位
├── DTO具体编码函数组成的SyncBindings
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

不存在：dto_sync_system、WebSocketMessageSend、ConnState副本、每帧同步调查、sync中转通道。
```
