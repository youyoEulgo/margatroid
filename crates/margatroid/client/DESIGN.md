Margatroid领域实现契约；共享Client槽位与直接同步投递已实现，运行进程需使用相应构建。

# 伪代码格式

```text
板块：一级标题，按职责列出core与plugin文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列字段、签名及直接行为；System列处理事件、闭包结果、提交闭包及产出事件。
属性不写Rust Attribute；持有关系使用树形图。
业务plugin之间不建立依赖；core仅暴露领域数据与能力。
```

# 板块格式

```text
# lib        client/core/src/lib.rs；client/plugin/src/lib.rs  查询扩展、共享槽位与安装
# components client/core/src/components.rs                    已注册Client身份引用
# events     client/core/src/events.rs                        注册和断连领域事件
# system     client/plugin/src/system.rs                      事件路由
# handler    client/plugin/src/handler.rs                     身份注册与销毁
# types pub  client/core/src/types.rs、shared.rs、output.rs、probe.rs、probe_factory.rs  共享槽位与同步能力
# error      client/core/src/error.rs                         注册、发送与编码错误
```

# lib

## 类型

公开，定义于client_core：
```text
WorldClientExt
    client_by_connection(&self, connection: ConnectionId) -> Option<(Entity, &Client)>
        行为：查询完整Client Entity及其不可变身份引用。
    clients(&self) -> Clients
        行为：克隆World中唯一Clients共享槽位，不克隆注册表内容，不把World带入异步线程。
    impl WorldClientExt for World

Clients：Client领域Resource及网络/异步任务共用能力
    inner: Arc<ClientSlots>
    default() -> Self
    same_instance(&self, other: &Self) -> bool
    allocate(&self) -> Result<(ConnectionId, ClientReceiver, CancellationToken), ClientError>
        行为：分配不复用ConnectionId、每连接唯一有界出站队列、取消能力及请求登记集合，插入尚未注册槽位。
    claim(&self, connection: ConnectionId, id: &str) -> Option<Arc<RequestTicket>>
        行为：登记连接内唯一在途id，ticket释放时移除；不维护第二张网络连接表。
    get(&self, connection: ConnectionId) -> Option<ClientSender>
    adopt(&self, connection: ConnectionId, sender: ClientSender) -> Result<(), ClientError>
        行为：显式接入已有传输能力，拒绝重复ConnectionId；生产WebSocket使用allocate。
    registered(&self, connection: ConnectionId) -> Option<Arc<ClientIdentity>>
        行为：读取唯一身份记录，未注册或已移除返回None。
    select(&self, targets: &[ClientSelector]) -> Vec<(ConnectionId, ClientSender)>
        行为：短读锁筛选当前已注册且未关闭/取消的槽位，匹配Type、Name或Broadcast，按ConnectionId去重；释放锁后才编码或入队。
    commit_registration(&self, connection: ConnectionId, identity: Arc<ClientIdentity>) -> Result<(), ClientError>
        行为：同一写锁验证槽位仍存在、sender未关闭、未注册及名称未占用，再同时发布身份与名称索引。
    remove(&self, connection: ConnectionId) -> bool
        行为：先从共享表和名称索引移除并取消该槽位，停止其后续选择；重复移除无操作。
    initialize(&self, connection: ConnectionId)
        行为：注册成功后为符合条件的现存状态源投递StateKey，读取当前值；不重放chunk/日志/MCL/历史事件。
    register_source(&self, key: SyncKey, targets: Arc<[ClientSelector]>, prepare: StatePrepare) -> SyncSource
        行为：登记owned状态读取源并分配epoch，替换同key旧来源先使其失效；立即向当前符合条件的连接排入一次该StateKey。
    publish_state(&self, source: &SyncSource, revision: u64)
        行为：检查来源存活及版本，缓存轻量来源信息；遍历当前select结果直接调用sender.try_send_state。
    send_ordered(&self, targets: &[ClientSelector], frame: Arc<ClientFrame>)
        行为：遍历当前select结果直接投递有序具体帧，不登记初始状态。
    send_scoped(&self, targets: &[ClientSelector], frame: Arc<ClientFrame>, active: Arc<AtomicBool>)
        行为：来源仍存活才选择当前接收者，发送项携带同一来源有效标记；writer取帧时复检，来源失效则丢弃旧帧。
    retire_source(&self, source: &SyncSource)
        行为：失效来源、移除其初始化入口及旧pending状态；不借助ECS事件或运行帧。
    impl Resource + Clone
        行为：World、DTO State、同步入口及异步监控克隆同一个Arc。

SyncContext<T>：状态同步能力
    clients: Clients
    source: SyncSource
    marker: PhantomData<fn() -> T>
    new(clients: Clients, key: SyncKey, targets: Arc<[ClientSelector]>, snapshot: impl Fn() -> Result<(u64, T), ClientPrepareError> + Send + Sync + 'static, encode: impl Fn(T) -> Result<Arc<ClientFrame>, ClientPrepareError> + Send + Sync + 'static) -> Self
        行为：将owned读取及编码封装为StatePrepare，执行时spawn_blocking并真实await；只捕获Weak领域数据，不把World带进异步端。
    publish(&self, revision: u64)
        行为：实际更新后调用Clients.publish_state，不固定发送器列表。
    close(&self)
        行为：Clients.retire_source失效当前来源并移出初始化索引。
    source(&self) -> &SyncSource
    impl Clone
UpdateProbe<T>
    callback: Arc<dyn Fn(T) + Send + Sync>
    new(callback: impl Fn(T) + Send + Sync + 'static) -> Self
    publish(&self, value: T)
        行为：所属领域唯一写入口在实际变化后提交typed派生纯值；回调由装配层绑定公共视图及Clients，不允许缺少回调。
ProbeFactory<T>
    callback: Arc<dyn Fn(Entity) -> UpdateProbe<T> + Send + Sync>
    new(callback: impl Fn(Entity) -> UpdateProbe<T> + Send + Sync + 'static) -> Self
    for_owner(&self, owner: Entity) -> UpdateProbe<T>
    impl Clone + Resource（T: Send + Sync + 'static）

```

公开，定义于client_plugin：
```text
ClientPlugin
    schedule: String
    new() -> Self
    impl Default
    with_schedule(self, schedule: impl Into<String>) -> Self
    impl Plugin
        build(self, app: &mut App)
            行为：检查运行计划和安装标记；复用已插入Clients，无槽位时安装唯一默认槽位，安装注册及断连System；不创建服务器或同步分发任务。
```

私有，定义于client_plugin：
```text
ClientPluginInstalled：安装标记，Resource
```

# components

## 类型

公开：
```text
Client：已注册客户端的World身份组件
    connection_id: ConnectionId
    identity: Arc<ClientIdentity>
    impl Component + Clone + Debug
    行为：identity与共享槽位引用同一个不可变对象；不另存可独立修改的type/name/sender副本。
```

# events

## 类型

公开，定义于client_core：
```text
ClientRegistrationReply = Arc<Mutex<Option<oneshot::Sender<Result<RegisteredClient, ClientError>>>>>
RegisterClient
    id: String
    connection_id: ConnectionId
    client_type: String
    name: Option<String>
    reply: ClientRegistrationReply
    impl Event + Clone + Debug
    行为：不再携带独立sender；真实发送能力从同一Clients槽位取得。
ClientDisconnected
    connection_id: ConnectionId
    impl Event + Clone + Debug
    行为：传输任务已移除共享槽位后恰好通知一次World身份清理，包括从未注册的连接；这是连接生命周期事件，不承担sync中转。
```

# system

## 函数

crate公开：
```text
client_registration_system(world: &mut World)
    处理事件：RegisterClient
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    oneshot回复：Result<RegisteredClient, ClientError>
    行为：逐条调用handle_client_registration。
client_disconnect_system(world: &mut World)
    处理事件：ClientDisconnected
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    行为：逐条调用handle_client_disconnect。
```

# handler

## 函数

crate公开：
```text
handle_client_registration(world: &mut World, request: &RegisterClient)
    行为：认领原reply，调用register_client；成功后initialize当前连接，再通过原oneshot回复；回复丢失时只清理本次注册。
handle_client_disconnect(world: &mut World, event: &ClientDisconnected)
    行为：幂等移除共享槽位，销毁对应Client Entity和Address；不撤销已受理命令。
```

私有：
```text
register_client(world: &mut World, request: &RegisterClient) -> Result<RegisteredClient, ClientError>
    行为：
        校验请求id、client_type、名称和当前真实槽位，名称缺省为连接id。
        检查Address未占用，创建Entity与不可变ClientIdentity并挂载Client及Address。
        调用Clients.commit_registration原子复检槽位、重复身份及名称；失败销毁本次Entity，不留下半注册身份。
        断连与注册交错时以共享槽位存在性和取消状态裁决，旧连接不能复活；返回原RegisteredClient纯值。
        initialize使用来源当前版本，后续publish同样按当前接收者筛选；二者并发不允许旧快照覆盖新版本。
valid_client_type(value: &str) -> bool
    行为：非空，只含小写ASCII字母、数字、下划线或短横线。
valid_client_name(value: &str) -> bool
    行为：非空，不是点或双点，不含控制字符、斜杠、反斜杠或冒号。
```

# types pub

## 类型

公开，定义于client_core：
```text
ConnectionId：不复用的newtype(u64)，Copy + Clone + Debug + Eq + Hash + Ord + Display
ClientIdentity
    client: Entity
    address: String
    client_type: String
    name: String
    impl Clone + Debug
ClientSelector
    Broadcast | Type(String) | Name(String)
    impl Clone + Debug + Eq
ClientSlots
    registry: RwLock<Registry>
    sources: Mutex<BTreeMap<SyncKey, SyncSource>>
Registry
    entries: BTreeMap<ConnectionId, ClientSlot>
    names: BTreeMap<String, ConnectionId>
ClientSlot
    sender: ClientSender
    identity: Option<Arc<ClientIdentity>>
    active: Arc<Mutex<BTreeSet<String>>>
    行为：cancel从sender内部取得，不另建第二个取消状态。
RequestTicket
    id: String
    connection: ConnectionId
    active: Arc<Mutex<BTreeSet<String>>>
    impl Drop：释放在途id
SyncKey
    topic: String
    object: String
SyncSource(Arc<StateSource>)
StateSource
    key: SyncKey
    epoch: u64
    active: AtomicBool
    revision: AtomicU64
    targets: Arc<[ClientSelector]>
    prepare: StatePrepare
    行为：key/epoch/active控制来源归属；revision仅在推进时发布，prepare只持Weak状态读取。
StatePrepare = Arc<dyn Fn() -> PrepareFuture + Send + Sync>
PrepareFuture = Pin<Box<dyn Future<Output = Result<PreparedState, ClientPrepareError>> + Send>>
PreparedState
    revision: u64
    frame: Arc<ClientFrame>
    行为：source由ClientReceiver.preparing保存，版本取实际读取快照时刻。
ClientFrame
    Text(String) | Binary(Vec<u8>) | Close { code: u16, reason: String }
OutputItem（私有）
    Ordered { frame: Arc<ClientFrame>, active: Option<Arc<AtomicBool>> }
    State(SyncKey)
ClientOutput
    pending: Mutex<BTreeMap<SyncKey, SyncSource>>
    cancel: CancellationToken
ClientSender
    sender: mpsc::Sender<OutputItem>
    output: Arc<ClientOutput>
    channel(capacity: usize) -> (ClientSender, ClientReceiver)
    try_send(&self, frame: ClientFrame) -> Result<(), ClientSendError>
    try_send_shared(&self, frame: Arc<ClientFrame>) -> Result<(), ClientSendError>
    try_send_scoped(&self, frame: Arc<ClientFrame>, active: Arc<AtomicBool>) -> Result<(), ClientSendError>
        行为：仅有界入队；Full/Closed立即取消连接，不能静默丢单个有序帧后继续。
    try_send_state(&self, source: SyncSource) -> Result<(), ClientSendError>
        行为：pending无key时排一个State项，已有key时只替换最新有效来源；无大快照副本或中转任务。
    cancel(&self)
    cancel_token(&self) -> CancellationToken
    is_closed(&self) -> bool
    impl Clone + Debug
ClientReceiver
    receiver: mpsc::Receiver<OutputItem>
    output: Arc<ClientOutput>--不拥有sender，最后发送能力释放后可检测真实队列结束
    preparing: Option<(SyncSource, PrepareFuture)>
    sent: BTreeMap<SyncKey, (u64, u64)>
    recv(&mut self) -> Future<Output = Option<ClientFrame>>
        行为：Ordered复检可选active，State取pending最新来源并在preparing保存Future；prepare完成后复检来源和已发送epoch/revision，旧内容跳过。
        取消本次recv Future不丢preparing，下一次继续原读取，不新建同次worker。
    try_recv(&mut self) -> Result<ClientFrame, mpsc::error::TryRecvError>
    close(&mut self) -> Future<Output = ()>
        行为：取消连接、关闭队列及清pending，等待已有preparing真实结束；不丢Future冒充worker结束。
    impl Drop：请求取消，正常网络退出先await close。

RegisteredClient
    client: Entity
    address: String
    client_type: String
    impl Clone + Debug + PartialEq + Eq
```

# error

## 类型

公开：
```text
ClientError
    InvalidRequest | InvalidClientType | InvalidName | DuplicateName
    DuplicateClient | ConnectionMissing | AddressConflict
ClientSendError
    Full | Closed
ClientPrepareError
    SourceClosed | EncodeFailed | TooLarge | ReadFailed
    行为：SourceClosed丢弃旧来源；其余错误诊断并按协议关闭受影响连接，不无限重试或唤醒World。
```

# 持有关系

```text
compose创建唯一Clients
├── World Resource
├── ClientPlugin
├── DTO State / Axum handler
├── 异步文件与记录监控
└── 同步领域统一更新入口
    └── SyncContext / scoped有序发送
        └── 同一个Arc<ClientSlots>
            ├── 未注册及已注册ClientSlot
            │   ├── 同一ClientIdentity Arc（与Client组件共享）
            │   └── ClientSender → 每连接唯一有界队列
            └── 可独立读取的状态来源索引（不保存每连接整份JSON副本）

Axum serve_connection
├── 唯一ClientReceiver
├── owned状态prepare / 具体有序帧
└── 唯一socket writer
```
