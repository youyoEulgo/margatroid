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
                取得装配层提供的WorkspaceUpdateContext与AgentUpdateContext，复用World唯一Clients，初始化公共视图和SyncObservations；存在ModelConfigFile时安装其原有来源监控。
                对安装前已有实体初始化一次探针来源；后续只由实际Installed、SourcesInstalled、Changed、Closing及Removed回调驱动，不逐帧查找。
                Workspace登记来自workspace_core数据库确认提交通知，不再建立workspace.yaml定义监控；目录加载由Workspace初始化System负责。
                注册GET /ws及GET/HEAD /{workspace}/{*path}；文件handler复用既有ingress、AxumLifetime及WorldClosureSender，不新增业务System或公共事件。
                只安装PRE_UPDATE的abort_control_system和UPDATE的dto_route_system，不安装同步System。
sync_bindings(clients: Clients) -> DtoSyncBindings
    行为：一次构造AgentUpdateContext及WorkspaceSyncBinding，资源工厂均从同一个AgentUpdateContext派生；任务绑定复用同一Clients但持独立集合来源表，不维护另一份聚合状态结构。
task_sync_binding(clients: Clients) -> TaskObserverBinder
    行为：捕获Arc<Mutex<BTreeMap<Entity, (TaskUpdateContext, SyncContext<AgentTaskSnapshot>)>>>；same_collection去重，替换旧来源时close，snapshot_source仅持集合Weak。
        observe的changed/retired回调发布或关闭来源；retired回调只持来源表Weak，实体编号复用不移除新的集合来源。
agent_sync_context(clients: Clients) -> AgentUpdateContext
    行为：创建AgentViews；AgentUpdateContext::new的World发布回调与public纯值回调共享同一AgentViews Arc。
        World回调更新Agent及turn快照；Installed安装任务和Tools观察，SourcesInstalled创建UI/记录/设置监控，SourcesClosing/Removed停止监控并返回完成等待。
resource_sync_bindings(context: AgentUpdateContext) -> ToolObserverFactory
    行为：for_owner取得context.scoped(owner)，将ToolEntry映射为AgentPublicUpdate::Tools交给同一public回调。
model_sync_binding(context: AgentUpdateContext) -> inference_core::ModelObserverFactory
sandbox_sync_binding(context: AgentUpdateContext) -> sandbox_resource_core::SandboxObserverFactory
    行为：按owner衍生scope，模型/列表/选择提交同一公共视图；来源关闭后旧scope回调失效。
workspace_sync_binding(clients: Clients) -> WorkspaceSyncBinding
    行为：创建独立WorkspaceViews；生命周期回调更新rows，Installed按database.same_connection去重并订阅changes；Closing移除watch并取消，Removed移除rows。
sync_selectors() -> Arc<[ClientSelector]>
    行为：返回仅含ClientSelector::Type("webui")的固定Arc数组；状态、任务、资源和持久记录与推理出口统一，不从配置或任意wire消息推导投递范围。

```

# system

## 函数

crate公开：
```text
dto_route_system(world: &mut World)
    处理事件：DtoRequest
    接收闭包结果：None
    产出事件：RegisterClient、CreateWorkspace、DeleteWorkspace、StartWorkspaceAgent、StopAgent、ArchiveAgent、MclCommand
    回复：各请求原oneshot
    行为：逐条受理普通请求，业务更新由所属领域统一入口直接同步，不在此轮询投影。

abort_control_system(world: &mut World)
    处理事件：AbortTurnControl
    接收闭包结果：None
    产出事件：None
    回复：Result<(), ProtocolErrorDto>
    行为：调用handle_abort_control，经Agent统一入口同步清turn，再仅对显式绑定该turn的推理执行冻结、对该turn的工具执行中止裁决；自由调用不参与回合中止；各领域入口直接发布实际变化。
```

# handler

## 函数

crate公开：
```text
workspace_file_handler(State(state): State<DtoState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, uri: OriginalUri, method: Method, path: Result<Path<(String, String)>, PathRejection>) -> Future<Output = Response>
    行为：GET/HEAD只读访问，不需要Agent、turn、MCL或预先注册WebSocket；使用现有ingress策略检查peer、Host和可选Origin，不将缺失Origin当成自动授权。
        workspace为encodeURIComponent(完整Workspace节点路径)对应的单个URL段，解码一次并通过World核对Workspace组件；不接受展示名称、定义文件路径、主机绝对路径或其他kind作为别名。
        通过state.world提交一次原World闭包，按完整Address取得实际Workspace，读取其project_root及生命周期；不存在404、Deleting409，Running/Error允许读取；不从重载定义的project_root代替仍在运行实例的实际工作目录。
        取得owned根路径后在持有AxumWorkGuard的受监督blocking读取中调用read_workspace_file；只允许工作目录内的普通文件，不产生写入、任务、历史或MCL审计；文件响应由同一Axum普通HTTP生命周期监督，不借出World或数据库。
        成功返回200、准确Content-Type、Content-Length、Cache-Control: no-store、X-Content-Type-Options: nosniff；GET原始字节流，HEAD相同响应头且无正文；不base64包装，不重定向到文件系统路径，不返回目录列表。
        路径/编码不合法400，越界/符号链接或权限拒绝403，Workspace/文件不存在或不是普通文件404，关闭中的Workspace409，服务停止503，其他读取失败500；非Unix目标在尚无安全锚定打开实现时返回501，不降级为不安全路径读取；错误正文为空，不泄露主机路径。
        初版不提供写入、目录浏览、自动文件监控、条件缓存或分段读取协议；收到Range仍按普通GET返回200全文件，不宣称206支持。
read_workspace_file(root: PathBuf, path: String, head_only: bool) -> Result<Response, StatusCode>
    行为：handler先核对原始URI编码，Path extractor只解码一次，本函数接收已解码路径，不再次百分号解码；拒绝非法编码、NUL、反斜杠、绝对路径、盘符及解码后.或..目录段；编码后的分隔符不能绕过分段校验。
        以实际project_root为锚点打开目录，逐段验证和打开，拒绝根之下的符号链接及特殊文件，最终文件始终位于该根内；检查与打开之间的路径替换也不能绕过边界，不以单次starts_with字符串检查代替文件归属验证。
        project_root自身按实际配置解析，文件读取不拼接数据根或Agent目录，不回退Driver UI或平台静态资源。
        Unix打开成功后取得该句柄长度，GET用tokio File及ReaderStream流式读取并take该长度，HEAD不构建文件Body；类型按扩展名静态映射，未知application/octet-stream。

validate_workspace_file_path(path: &str) -> Result<(), StatusCode>
    行为：空路径、绝对路径、反斜杠、NUL、盘符、空段及.段400，..段403；仅校验已经解码一次的相对路径，不再次解码。
open_workspace_file(root: &Path, path: &str) -> Result<File, StatusCode>--Unix
    行为：打开根目录句柄，按路径段以fstatat(AT_SYMLINK_NOFOLLOW)核对类型，再以openat(O_NOFOLLOW|O_CLOEXEC|O_NONBLOCK)逐级打开，目录段额外要求O_DIRECTORY；拒绝符号链接及特殊文件，不读FIFO或设备；最终再次确认普通文件。
workspace_file_status(error: io::Error) -> StatusCode
    行为：不存在/非目录404，权限拒绝及ELOOP403，其余IO失败500；不把主机路径写进HTTP正文。

ws_handler(State(state): State<DtoState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Future<Output = Response>
    行为：评估入口策略和退出状态，通过state.clients.allocate取得真实连接、唯一ClientReceiver及取消能力；升级任务持有AxumWorkGuard。

handle_dto_request(world: &mut World, request: DtoRequest)
    行为：取出具体owned对象，resource.req交给resources进行异步只读查询，其余按route转换领域请求或只读查询；原reply继续沿领域执行链回复；agent.start/stop/archive按新事件分派，不接受restart旧接口。

handle_abort_control(world: &mut World, event: AbortTurnControl)
    行为：认领原请求，校验真实Client、Agent及turn，调用WorldAgentExt的统一turn入口及相关领域取消能力；原oneshot回复。

projection::address(world: &World, entity: Entity) -> Option<String>
projection::workspaces(world: &World) -> Vec<WorkspaceInfoDto>
projection::agents(world: &World) -> Vec<AgentStateDto>
    行为：读取当前Agent组件的workspace/state/turn及已挂载的Tools、ToolTasks、推理公共信息；状态来自Starting/Running/Closing，不从目录类别或任务returned推断运行状态。turn独立发布，creator取Agent.parent地址和task_id，运行会话取Agent.instance，不依赖旧任务绑定组件；error仅从parent任务的公开错误投影，Agent自身不新增fault字段。
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
refresh_ui_watch(world: &mut World, entity: Entity, ui_path: Option<PathBuf>) -> Vec<AgentObservationWait>
refresh_record_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
    行为：只在SourcesInstalled回调绑定实际AgentStorage及显式ui_path；路径和数据库共享身份相同则不重复建立。先订阅真实来源再取初始快照，旧来源失效、取消并返回真实完成等待，新来源持AxumWorkGuard。
        ui_path来自Driver创建注入方法的可信宿主材料，不从tool_path、Workspace目录或配置内容猜测；持久化来源从实际AgentStorage取得。
        read、req/res与重复ready不创建监控，SourcesClosing必须等待已返回的完成句柄。
stop_ui_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
stop_record_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
refresh_settings_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
stop_settings_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait>
    行为：移除并失效对应source、请求取消，返回Completion等待；Agent生命周期真实await之后才能宣告资源释放。
run_ui_watch(context: UiMonitorContext) -> Future<Output = Result<(), DtoAsyncError>>
    行为：先挂notify观察再初读实际ui_path，覆盖文件及目录新增、删除和原子替换；Access和无关路径忽略，连续构建变更在安静窗口合并。
        稳定变化由blocking worker读取完整UI集合，更新版本化UiDelivery并经同一SyncContext直投StateKey，重复revision或相同错误不发布。
        只监控本次实际UI目录，不复制或回退旧来源；取消后仍等待当前文件worker和monitor真实结束，再完成MonitorCompletion。
run_record_watch(context: RecordMonitorContext) -> Future<Output = Result<(), DtoAsyncError>>
    行为：先取得record_changes接收端和当前真实高水位作为初始position，不自动重放旧历史；等待实际COMMIT的watch高水位，只对新增sequence执行有界索引查询。
        通知合并时持续按真实sequence追齐，所有批次保持升序；encode_batch生成有序帧，经同一Clients.send_scoped直接选择当前接收者，追齐后等待，不产出ECS同步事件。
        取消时停止新读取，等待在途blocking读取结束后完成monitor；来源替换使用独立active标记，旧批次不能投递到新实例。
read_batch(storage: &AgentStorage, position: RecordWatermarks, high: RecordWatermarks) -> Result<RecordBatch, StorageError>
encode_batch(address: &str, batch: &RecordBatch) -> Result<Vec<Arc<ClientFrame>>, DtoAsyncError>
    行为：每类最多50条，当前高水位限定排他范围；有request_id的assistant/error由推理通道完成，记录监控跳过其网络投影但照常推进游标；compact历史虽保留request_id但不属于assistant生成去重分支，仍交付generation=null的持久记录；非生成消息full保留真实持久序号、generation=null，MCL和日志过滤内部VM信息，完整编码/预算失败明确重试诊断而不重复旧记录。
install_model_config_monitor(world: &mut World)
    行为：取得实际ModelConfigFile；无来源时不创建伪监控。使用同一Clients及固定webui目标建立model.config.sync的state_source，来源view与SyncContext同归ModelConfigSource；创建持AxumWorkGuard的watch_model_config。
watch_model_config(source: ModelConfigView) -> Future<Output=Result<(),String>>
    行为：notify观察文件父目录NonRecursive，只接受目标来源路径的实际变更及观察错误，忽略Access和其他文件；先挂观察再初读，AtomicBool/Notify合并脏信号，顺序await read，闲时等待；取消仍等待当前文件worker完成，不产出ECS同步事件或逐帧调查。
ModelConfigView::read(&self) -> Future<Output=Result<String,ProtocolErrorDto>>
ModelConfigView::replace(&self, content:String) -> Future<Output=Result<(),ProtocolErrorDto>>
ModelConfigView::refresh_locked(&self) -> Future<Output=Result<String,ProtocolErrorDto>>
    行为：operations锁统一串行监控/查询/修改；调用inference的read/load能力取得原文与最近校验路由，通过既有WorldClosureSender将ModelRoutes::replace和AgentInferenceConfig安全投影提交回World；无效来源清空路由投影，不回退旧模型。
        原文读取成功即可供编辑，不要求当前文本语义有效；读取失败发布content=None及error。视图实际变化经update直接提交SyncContext，初始None/None表示尚未读取。
        replace先await inference的完整校验和原子写入能力，成功后refresh_locked，再完成请求；校验/写入失败返回原错误，不伪造成功。
resources(world: &World, object: Box<dyn Any + Send>)
    行为：认领Prepared<ResourceQueryDto, Vec<ResourceInfoDto>>，校验真实已注册Client，克隆address::ResourceRoots及AxumWorkGuard。
        以AsyncRuntimeHandle::spawn提交闭包，再用spawn_blocking调用address::query_resources；将owned条目转为协议字段，显式提供nullable tool_path。
        目录读取、worker或UTF-8路径转换失败通过原reply返回ResourceReadFailed并省略resources；成功返回完整列表，没有匹配为[]。
        World只克隆值，不进行扫描；闭包不借用World、不创建任务或订阅，网络回复不需要另一次World帧。
model_config(world: &World, object: Box<dyn Any+Send>, write:bool)
    行为：按req/update取Prepared，校验真实注册Client，克隆ModelConfigView并在持AxumWorkGuard的普通异步闭包执行read/replace，原oneshot回复；不创建Workspace/Agent/turn或新领域事件。
install_workspace_monitor(world: &mut World)
    行为：DTO首次安装时，对已有Workspace调用Installed并发布当前rows；后续依赖领域探针，不按帧扫描。
Workspace Installed回调中的spawn异步闭包
    行为：subscribe_changes后先borrow_and_update当前确认快照，再等待changes.changed/cancel/lifetime；只更新当前owner的登记列表并publish，不修改运行状态字段。
        WorkspaceWatch保存实际database、active及cancel；Closing/Removed移除watch时先失效active再cancel。工作Future持AxumWorkGuard，退出后释放。
run_settings_watch(context: SettingsContext) -> Future<Output = ()>
    行为：绑定实际AgentStorage的settings_path，目录不存在时观察可用的父来源并报告读取状态；观察父目录以覆盖原子替换、删除及恢复；先订阅再读取，过滤Access与无关路径，读取在blocking worker执行。
        配置内容是Driver自定义值，仅通过storage_core既有CONFIG读取入口做通用语法验证和发布；不推断alias、模型、归档或启动规则，不代替Driver热更新执行。
        当前消息目录没有配置正文sync，本监控只记录来源读状态及去重的tracing诊断，不新增网络消息或持久配置缓存；CONFIG查询读取当前真实来源，不靠监控缓存代替。
        Agent关闭或来源替换先取消，再等待当前worker和监控实际完成。

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
        workspace.create/delete按原reply转交CreateWorkspace/DeleteWorkspace；agent.start按Existing/New构造StartWorkspaceAgent<ToolExecuteResponse, ToolError>并原样转交reply，agent.stop/archive对应StopAgent/ArchiveAgent。
        不再接收workspace.start/stop/restart/update/rename；agent.start只发起Driver工具调用，不在网络层准备Agent持久化组件或创建实体。
        客户端断开仅结束网络等待，已受理业务由所属监督收口，不能丢弃已打开连接或待绑定实体。
        UI查询沿原oneshot返回观察状态；base64和wire编码保持在网络边界。

request<T, R, E>(ticket: Arc<RequestTicket>, route: &str, input: T, response: &str, project: impl FnOnce(R) -> Result<Value, ProtocolErrorDto> + Send + 'static) -> (DtoRequest, ReplyFuture)
    行为：原接收端与纯投影组成ReplyFuture，原发送端交给具体领域。
dispatch<T: IntoEvent>(world: &mut World, object: Box<dyn Any + Send>)
    行为：具体TryFrom后发送领域请求事件；这是业务请求受理，不承担sync中转。
query<T, R>(world: &World, object: Box<dyn Any + Send>, read: impl FnOnce(&World, Entity, &T) -> Result<R, ProtocolErrorDto>)
    行为：校验真实Client及目标后只读并完成原reply。
records(world: &World, route: &str, object: Box<dyn Any + Send>)
    行为：克隆实际AgentStorage，由worker调用对应分页core函数，通过原reply返回，不建立监控。

query_agent_ui(world: &World, object: Box<dyn Any + Send>)
    行为：按实际Agent实体读取SourcesInstalled绑定的UI来源已有观察状态；未就绪返回AgentUiNotReady，不依赖旧AgentResources组件，不创建监控。

project_agent_ui(value: Option<Arc<DriverUiBundle>>) -> Result<Option<AgentUiBundleDto>, ProtocolErrorDto>
    行为：保持原字节、排序相对路径、媒体类型、entry及内容revision，data使用标准base64。
check_ui_budget(address: &str, ui: &Option<AgentUiBundleDto>) -> Result<(), ProtocolErrorDto>
    行为：同时检查完整sync及最大合法id的res信封，超出映射DriverUiTooLarge。
build_ui_delivery(address: &str, state: DriverUiState) -> UiDelivery
    行为：投影完整稳定UI集合或错误并编码一次，内容key去重；内部单调版本负责时序，摘要revision不作时间排序。
client(world: &World, connection: ConnectionId) -> Result<Entity, ProtocolErrorDto>
target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
agent(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
    行为：使用实际连接身份与Address定位完整Entity；不解析网络Agent生命周期参数。
deliver<R, E>(reply: &PreparedReply<R, E>, result: Result<R, E>)
reject<T, R, E: BoundaryError>(request: Prepared<T, R, E>, failure: ProtocolErrorDto)
    行为：只认领原oneshot发送端一次。
encode_response(value: Value) -> Option<String>
invalid_reply(text: &str) -> Option<Value>
frame(kind: &str, id: &str, result: Result<Value, ProtocolErrorDto>) -> Value
mcl_result(value: MclValue) -> Result<Value, ProtocolErrorDto>
    行为：Turn(id)投影为data={kind:"turn",value:id}，保留原请求id；Unit仍无data，不能把turn结果丢弃或投影为Unit。
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
TaskObserverBinder = Arc<dyn Fn(Entity, String, &tool_core::ToolTasks) + Send + Sync>
TaskSyncInstaller(TaskObserverBinder)：Resource + Clone
ToolChangeObserver = Arc<dyn Fn(Vec<(String, tool_resource_core::ToolEntry)>) + Send + Sync>
ToolObserverFactory：Clone，非Resource
    bind: Arc<dyn Fn(Entity) -> ToolChangeObserver + Send + Sync>
    for_owner(&self, owner: Entity) -> ToolChangeObserver
DtoSyncBindings
    agent: AgentUpdateContext
    workspace: workspace_core::WorkspaceUpdateContext
    workspace_binding: WorkspaceSyncBinding
    tasks: TaskObserverBinder
    tools: ToolObserverFactory
    models: inference_core::ModelObserverFactory
    sandboxes: sandbox_resource_core::SandboxObserverFactory
WorkspaceSyncBinding：Resource + Clone
    updates: workspace_core::WorkspaceUpdateContext
    views: Arc<WorkspaceViews>

```

私有：
```text
ModelFileSnapshot
    content: Option<String>
    error: Option<ProtocolErrorDto>
ModelConfigSource
    file: ModelConfigFile
    view: View<ModelFileSnapshot>
    sync: SyncContext<ModelFileSnapshot>
    operations: tokio::sync::Mutex<()>
    world: WorldClosureSender
    lifetime: AxumLifetime
    Drop：关闭sync来源
ModelConfigView(Arc<ModelConfigSource>)--Resource、Clone
DtoState
    world: WorldClosureSender--文件请求沿既有UPDATE闭包通道一次取得实际Workspace根，不缓存第二份目录
    ingress: IngressPolicy
    events: Arc<OnceLock<AsyncContext>>--只用于业务请求和连接生命周期
    clients: Clients
    lifetime: AxumLifetime
    impl Resource + Clone
ReplyOutput
    value: Value
    request: Value
ReplyFuture = Pin<Box<dyn Future<Output = ReplyOutput> + Send>>
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
IntoEvent
    type Event: core_plugin::Event
    into_event(self, world: &World) -> Result<Self::Event, ()>
RegisterClientDto(Prepared<RegisterConnectionDto, RegisteredClient, ClientError>)
CreateWorkspaceDto(Prepared<WorkspaceCreateDto, Entity, WorkspaceError>)
DeleteWorkspaceDto(Prepared<WorkspaceTargetDto, (), WorkspaceError>)
AgentStartDto(Prepared<AgentStartInputDto, tool_core::ToolExecuteResponse, tool_core::ToolError>)
    行为：Existing和New都经workspace领域；New的node_name/alias在arguments内，不重复追加顶层字段。
AgentStopDto(Prepared<AgentTargetDto, (), agent_core::AgentError>)
AgentArchiveDto(Prepared<AgentArchiveInputDto, (), workspace_core::WorkspaceError>)
MclCommandDto(Prepared<MclExecuteDto, MclValue, MclError>)
    impl TryFrom<Box<dyn Any + Send>> + IntoEvent
BoundaryError
    boundary(value: ProtocolErrorDto) -> Self
    external(self) -> ProtocolErrorDto
SyncObservations：Resource
    records: BTreeMap<Entity, RecordWatchSlot>
    ui: BTreeMap<Entity, UiWatchSlot>
    行为：只持监控归属和查询能力，不保存每连接全量投影或每帧发送位置。
SettingsObservations(BTreeMap<Entity, SettingsSlot>)：settings模块私有Resource
SettingsSlot
    storage: AgentStorage
    path: PathBuf
    cancel: CancellationToken
    completion: Arc<MonitorCompletion>
    impl Drop：cancel；完成由monitor实际退出通知。
SettingsContext
    storage: AgentStorage
    path: PathBuf
    cancel: CancellationToken
    lifetime: AxumLifetime
    guard: AxumWorkGuard
    completion: MonitorCompletionGuard
UiWatchSlot
    completion: Arc<MonitorCompletion>
    ui_path: PathBuf
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
    rows: Mutex<BTreeMap<Entity, WorkspaceInfoDto>>
    view: View<Vec<WorkspaceInfoDto>>
    sync: SyncContext<Vec<WorkspaceInfoDto>>
    sources: Mutex<BTreeMap<Entity, WorkspaceWatch>>
WorkspaceWatch
    database: workspace_core::WorkspaceDatabase
    active: Arc<AtomicBool>
    cancel: CancellationToken
    impl Drop：active=false并cancel；异步循环只捕获WorkspaceViews Weak。
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
│   ├── Workspace组件探针及数据库确认提交监控
│   ├── Agent/Turn/Tools/Sandbox/Model公共信息统一入口
│   ├── Task创建、输出、Return、错误、移除统一入口
│   ├── 创建时传入的UI目录磁盘监控
│   ├── 成功提交驱动的持久记录监控
│   └── 推理chunk/full执行链
└── 所有发送源直接遍历Clients并投递ClientSender
    └── 每连接唯一有界出站队列 → Axum唯一writer → WS

World DTO
├── dto_route_system：业务请求
└── abort_control_system：中止控制

已删除：dto_sync_system、WebSocketMessageSend、ConnState副本、每帧同步调查、sync中转通道。
```
