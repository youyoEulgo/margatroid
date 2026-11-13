Margatroid领域目标设计；本轮同步架构尚未实现，实施状态及契约变化见README。

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
# types pub  client/core/src/types.rs                         共享发送槽位、连接及同步发布能力
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
    new() -> Self
    allocate(&self) -> Result<(ConnectionId, ClientReceiver, CancellationToken), ClientError>
        行为：分配不复用ConnectionId、每连接唯一有界出站队列、取消能力及请求登记集合，插入尚未注册槽位。
    sender(&self, connection: ConnectionId) -> Option<ClientSender>
        行为：按真实传输连接取当前发送能力。
    claim(&self, connection: ConnectionId, id: &str) -> Result<RequestTicket, ClientError>
        行为：登记连接内唯一在途id，ticket释放时移除；不维护第二张网络连接表。
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
        行为：登记可独立异步读取的状态源，分配本次来源epoch；替换同key旧来源时先失效旧source。
    publish_state(&self, source: &SyncSource, revision: u64)
        行为：检查来源存活及版本，缓存轻量来源信息；遍历当前select结果直接调用sender.try_send_state。
    register_stream(&self, key: SyncKey, targets: Arc<[ClientSelector]>) -> SyncStreamSource
        行为：建立有序增量的来源租约，不放入初始状态索引，也不要求伪造snapshot回调。
    send_ordered(&self, source: &SyncStreamSource, frame: Arc<ClientFrame>)
        行为：检查active及accepting，遍历当前select结果，附带后端来源租约直接投递具体有序帧，不缓存为初始状态。
    retire_source(&self, source: &SyncSource)
        行为：失效来源、移除其初始化入口及旧pending状态；不借助ECS事件或运行帧。
    impl Resource + Clone
        行为：World、DTO State、同步入口及异步监控克隆同一个Arc。

SyncContext<T>：领域更新入口必需的状态同步上下文
    clients: Clients
    source: SyncSource
    snapshot: Arc<dyn Fn() -> SnapshotFuture<T> + Send + Sync>
    encode: Arc<dyn Fn(T) -> Result<Arc<ClientFrame>, ClientPrepareError> + Send + Sync>
    publish(&self, revision: u64)
        行为：实际可见状态发生变化后直接调用Clients.publish_state；snapshot/encode封装为该source的StatePrepare，不访问World。
    close(&self)
        行为：失效该来源，阻止已取消或旧实例的迟到更新。
    impl Clone
        行为：持有共享槽位及owned读取能力，不固定接收者列表。

StreamContext<T>：具体增量消息的同步上下文
    clients: Clients
    source: SyncStreamSource
    encode: Arc<dyn Fn(T) -> Result<Arc<ClientFrame>, ClientPrepareError> + Send + Sync>
    send(&self, update: T) -> Result<(), ClientPrepareError>
        行为：把真实chunk或持久记录投影编码一次后直接调用Clients.send_ordered；保持来源内顺序，不合并、不回World。
    finish(&self)
        行为：仅关闭accepting，不抹除已入队的完整终态。
    retire(&self)
        行为：来源实际结束/替换时失效active，取消迟到旧更新。
```

公开，定义于client_plugin：
```text
ClientPlugin
    schedule: String
    clients: Clients
    new(clients: Clients) -> Self
    with_schedule(self, schedule: impl Into<String>) -> Self
    impl Plugin
        build(self, app: &mut App)
            行为：检查运行计划和唯一共享槽位，插入同一Clients Resource及安装标记，安装注册及断连System；不创建服务器或同步分发任务。
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
    registry: RwLock<ClientRegistry>
    sources: Mutex<BTreeMap<SyncKey, SyncSource>>--仅当前状态源，读取回调只捕获Weak领域数据，防止Arc环
ClientRegistry
    entries: BTreeMap<ConnectionId, ClientSlot>
    names: BTreeMap<String, ConnectionId>--与entries在同一写锁提交
ClientSlot
    sender: ClientSender
    identity: Option<Arc<ClientIdentity>>
    cancel: CancellationToken
    active: Arc<Mutex<BTreeSet<String>>>
RequestTicket
    id: String
    connection: ConnectionId
    active: Arc<Mutex<BTreeSet<String>>>
    impl Drop：释放在途id

SyncKey
    topic: String--具体sync接口名
    object: String--集合或Agent Address等业务对象键
SyncSource
    key: SyncKey
    epoch: u64--后端来源租约，非网络字段；集合来源归应用，per-Agent来源归实际实例
    active: Arc<AtomicBool>
    revision: Arc<AtomicU64>
    targets: Arc<[ClientSelector]>
    prepare: StatePrepare--owned状态读取及编码，无World借用
SyncStreamSource
    key: SyncKey
    epoch: u64
    active: Arc<AtomicBool>--来源/实例生命周期，替换或停止时失效
    accepting: Arc<AtomicBool>--当前生产链是否仍接受新更新
    targets: Arc<[ClientSelector]>
    行为：正常生成结束只关闭accepting，已入队帧仍交付；来源替换或Agent停止使active失效，writer丢弃旧来源迟到帧。
StatePrepare = Arc<dyn Fn() -> PrepareFuture + Send + Sync>
PrepareFuture = Pin<Box<dyn Future<Output = Result<PreparedState, ClientPrepareError>> + Send>>
SnapshotFuture<T> = Pin<Box<dyn Future<Output = Result<(u64, T), ClientPrepareError>> + Send>>
PreparedState
    key: SyncKey
    epoch: u64
    revision: u64
    frame: Arc<ClientFrame>
    行为：版本取实际读取快照时刻，不把较旧通知版本当作实际内容版本。

ClientFrame
    Text(String) | Binary(Vec<u8>) | Close { code: u16, reason: String }
ClientOutputItem
    Ordered { source: Option<SyncStreamSource>, frame: Arc<ClientFrame> }
    StateKey(SyncKey)
ClientOutput
    sender: mpsc::Sender<ClientOutputItem>
    pending: Mutex<BTreeMap<SyncKey, SyncSource>>
    cancel: CancellationToken
ClientSender
    output: Arc<ClientOutput>
    try_send(&self, frame: ClientFrame) -> Result<(), ClientSendError>
    try_send_ordered(&self, source: SyncStreamSource, frame: Arc<ClientFrame>) -> Result<(), ClientSendError>
        行为：直接入队具体帧，保持顺序；Full/Closed明确返回并取消慢连接，不静默丢chunk或记录。
    try_send_state(&self, source: SyncSource) -> Result<(), ClientSendError>
        行为：只为同连接同key的首个pending状态排入StateKey，后续修改只更新该pending来源/版本，不排入多份大快照。
    is_closed(&self) -> bool
    impl Clone + Debug
ClientReceiver
    receiver: mpsc::Receiver<ClientOutputItem>
    output: Arc<ClientOutput>
    recv(&mut self) -> Future<Output = Option<ClientOutputItem>>
    take_state(&self, key: &SyncKey) -> Option<SyncSource>
        行为：取走最新pending来源，释放锁后prepare；更新期间可再排一个新StateKey。
    行为：每连接仅此一个有界队列，pending是本队列内部状态合并表，不是额外sync通道或后台分发器。

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
    └── SyncContext / StreamContext
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
