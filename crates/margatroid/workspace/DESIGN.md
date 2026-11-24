# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，不创建空模块。
类型、函数使用二级标题；列出字段、签名及直接行为。
System分别列出处理事件、接收闭包结果、提交闭包与产出事件，使用Rust类型和函数名；空项使用None。
不写Rust Attribute；持有关系使用树形图。
```

# 板块格式

```text
# lib        workspace/core/src/lib.rs；workspace/plugin/src/lib.rs  Workspace Resource与扩展；Plugin安装
# components workspace/core/src/components.rs                        Workspace与执行器组件
# events     workspace/core/src/events.rs                            工作区生命周期契约
# system     workspace/plugin/src/system.rs                          事件收集与路由
# handler    workspace/plugin/src/handler.rs                         生命周期编排
# types      workspace/plugin/src/types.rs                           闭包参数与输出
# types pub  workspace/core/src/types.rs                             状态、定义与线程能力
# error      workspace/core/src/error.rs                             Workspace错误
```

# lib

## 类型

公开，定义于workspace_core：
```text
WorkspaceConfig：工作区运行配置，公开Resource
    root: PathBuf--Workspace目录根路径
    impl Resource for WorkspaceConfig

WorkspaceUpdateContext：Workspace组件生命周期探针，公开Resource
    publish: Arc<dyn Fn(&mut World, Entity, WorkspaceUpdateKind) + Send + Sync>--由装配方注入，core不依赖客户端
    new(publish: impl Fn(&mut World, Entity, WorkspaceUpdateKind) + Send + Sync + 'static) -> Self
    notify(&self, world: &mut World, owner: Entity, kind: WorkspaceUpdateKind)
        行为：同步调用注入回调；组件安装、实际状态修改和实体移除均由plugin的统一入口调用，不发送同步事件。
    impl Resource + Clone

WorkspacePendingWork：初始化加载及已受理创建的完成观察，独立Resource
    shared: Arc<Mutex<WorkspacePendingState>>
    begin_load(&self) -> Option<WorkspaceWorkToken>
        行为：只受理一次初始化加载；首次提交前load_completed=false，重复请求返回None。
    begin_create(&self) -> WorkspaceWorkToken
        行为：登记一项已认领原reply的创建；不拥有World或数据库连接。
    load_completed(&self) -> bool
    is_idle(&self) -> bool
        行为：加载已完成且创建计数为零时才为true；应用退出必须等待此条件并清理后续安装的Workspace。
    impl Resource + Clone + Default
WorkspacePendingState：core私有
    load_started: bool
    load_completed: bool
    creates: usize
WorkspaceWorkToken：core公开不透明句柄
    shared: Arc<Mutex<WorkspacePendingState>>
    load: bool
    impl Drop
        行为：load标记加载完成，否则减少创建计数；plugin必须在World确认前序StartWorkspace已处理后释放，不因异步任务完成就提前宣告安装完成。
```

公开，定义于workspace_plugin：
```text
WorkspacePlugin：工作区插件
    root: PathBuf--插件安装时提供的Workspace目录根路径
    impl Plugin for WorkspacePlugin
        build(self, app: &mut App)
            行为：校验root为绝对路径，写入独立的WorkspaceConfig及WorkspacePendingWork资源；在只执行一次的RuntimePlugin::STARTUP安装load_workspace_system，在RuntimePlugin::UPDATE安装create_workspace_system、start_workspace_system、delete_workspace_system、start_agent_system和archive_agent_system。
```

# components

## 类型

公开：
```text
Workspace：工作区组件，定义于workspace_core
    alias: String--展示名称
    project_root: PathBuf--工作区基础工作目录，供Driver上下文使用
    state: WorkspaceState--当前实体的运行状态，不写入数据库
    database: WorkspaceDatabase--该Workspace数据库的访问句柄，记录所属的全部Agent
    set_state(&mut self, state: WorkspaceState) -> bool
        行为：相同状态返回false；实际修改后返回true，由plugin释放组件借用后调用WorkspaceUpdateContext::notify(Changed)。
```

# events

## 类型

公开：
```text
CreateWorkspace：工作区文件创建事件，定义于workspace_core
    address: String--工作区的完整节点地址
    alias: String--展示名称
    project_root: PathBuf--工作区基础工作目录
    reply: Arc<Mutex<Option<oneshot::Sender<Result<Entity, WorkspaceError>>>>>
    impl Event for CreateWorkspace

StartWorkspace：工作区启动事件，定义于workspace_core
    address: String--工作区实体的完整节点地址，由workspace_plugin使用address校验
    alias: String--展示名称
    project_root: PathBuf--工作区基础工作目录
    database: WorkspaceDatabase--已打开的工作区数据库句柄
    reply: Option<Arc<Mutex<Option<oneshot::Sender<Result<Entity, WorkspaceError>>>>>>--创建请求转交，加载为None
    impl Event for StartWorkspace

DeleteWorkspace：工作区删除事件，定义于workspace_core
    workspace: Entity--待删除的Workspace实体
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), WorkspaceError>>>>>--原请求的唯一回复端
    impl Event for DeleteWorkspace

StartWorkspaceAgent<R, E>：启动Driver工具调用的事件枚举，定义于workspace_core
    Existing
        workspace: Entity--所属Workspace实体，指定查询的数据库
        node_name: String--该Workspace内已登记Agent的单段相对节点名
        reply: Arc<Mutex<Option<oneshot::Sender<Result<R, E>>>>>
    New
        workspace: Entity--发起工具调用的Workspace实体
        tool_path: PathBuf--实际Driver工具入口路径
        arguments: serde_json::Value--原样传给Driver的工具调用参数
        reply: Arc<Mutex<Option<oneshot::Sender<Result<R, E>>>>>
    impl<R: Send + 'static, E: Send + 'static> Event for StartWorkspaceAgent<R, E>

ArchiveAgent：工作区Agent归档事件，定义于workspace_core
    workspace: Entity--所属Workspace实体，指定查询的数据库
    node_name: String--该Workspace内的单段相对节点名
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), WorkspaceError>>>>>--归档完成的唯一回复端
    impl Event for ArchiveAgent
```

# system

## 函数

crate公开：
```text
create_workspace_system(world: &mut World)
    处理事件：CreateWorkspace
    接收闭包结果：None
    提交闭包：create_workspace(root: PathBuf, event: CreateWorkspace, events: EventSender) -> Future<Output = ()>
    产出事件：None
    行为：收到CreateWorkspace后认领唯一reply，取得WorkspaceConfig.root、事件发送器及begin_create操作token，使用AsyncRuntimeHandle::spawn提交create_workspace异步闭包。
        闭包先发送StartWorkspace或原失败reply，再发送私有WorkspaceWorkCompleted携带token，不能直接在异步任务末尾释放token。

load_workspace_system(world: &mut World)
    处理事件：None
    接收闭包结果：None
    提交闭包：load_workspaces(root: PathBuf, events: EventSender) -> Future<Output = ()>
    产出事件：None
    行为：在初始化schedule执行一次，调用WorkspacePendingWork.begin_load取得token，取得root和发送器后提交load_workspaces异步闭包。
        所有StartWorkspace发出后，无论扫描成功或失败都发送携带token的私有WorkspaceWorkCompleted。

start_workspace_system(world: &mut World)
    处理事件：StartWorkspace
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    行为：先接收并处理全部StartWorkspace，再接收私有WorkspaceWorkCompleted，take并释放其中token，使完成观察晚于实体装配或明确失败；不新增公开业务事件或额外System。

delete_workspace_system(world: &mut World)
    处理事件：DeleteWorkspace
    接收闭包结果：Result<WorkspaceCleanupResult, WorkspaceError>
    提交闭包：Future<Output = Result<WorkspaceCleanupResult, WorkspaceError>>
    产出事件：None
    行为：接收DeleteWorkspace，校验目标Workspace及其管理目录，目录必须位于WorkspaceConfig.root内，且不包含project_root；仅Running或Error状态可受理删除，记录目录后将状态置为Deleting；Deleting状态拒绝重复删除；受理失败通过原reply直接回复错误。
        状态实际转为Deleting后，通过WorkspaceUpdateContext::notify(Changed)发布变化。
        同步关闭Workspace.database.close_admission()及ToolTasks.close_admission()，同时禁止直接Path工具请求和Lua新建Agent受理。
        克隆ToolTasks、数据库及原reply交给异步闭包；先取消所有已受理任务并等待finish()，再等待database.wait_idle()确认在途创建及归档凭证全部释放。
        任一任务清理失败时仍等待本轮其他已请求关闭项收口，再返回清理错误，不继续wait_idle或关闭数据库。
        任务必须将已受理Agent创建监督收口，取消不会丢失StartAgent的reply或遗留未绑定实体。
        清理完成后返回携带完整Entity及原reply的闭包结果，不跨await持有World或组件借用。
        System接收成功结果后，复核完整Entity仍对应原Workspace且状态为Deleting，同步调用Workspace.database.close()关闭数据库；此时无在途操作，若受理计数非零则返回Busy而不阻塞World。
        数据库关闭前调用WorkspaceUpdateContext::notify(Closing)，使该Workspace的数据库和任务同步来源失效；数据库关闭通知须使观察者结束，不等待需由World处理的额外轮询。
        数据库关闭成功后销毁实体，释放其所有组件，调用WorkspaceUpdateContext::notify(Removed)，再删除已记录的管理目录；不删除project_root或共享资源。
        任务清理或数据库关闭失败时保留实体及目录，将状态置为Error并通过原reply返回错误，不恢复任务受理；调用WorkspaceUpdateContext::notify(Changed)发布实际Error状态，可再次收到删除事件后重试。
        目录删除失败时实体已经销毁，通过原reply返回包含entity_removed=true和已校验管理路径的错误；不能再按已销毁Entity重试，不自动重建实体，残留目录须显式修复。
        实体销毁且目录删除完成后，通过原reply返回Ok(())；所有成功或失败分支均take唯一Sender，只回复一次，不另发删除完成事件。

start_agent_system(world: &mut World)
    处理事件：StartWorkspaceAgent<tool_core::ToolExecuteResponse, tool_core::ToolError>
    接收闭包结果：None
    提交闭包：None
    产出事件：tool_core::ToolExecuteRequest::Path
    行为：接收StartWorkspaceAgent并检查Workspace Running；Existing按Workspace内唯一node_name读取普通记录并调用database.ensure_startable(id)，New使用请求的tool_path和arguments。
        Driver工具参数是对象，专用node_name、alias及相对工作目录path由Driver参数提供；node_name缺省或null为临时，提供时必须为非空单段名且不能使用保留前缀~temporary-。
        New若arguments.node_name已登记则拒绝；Existing使用数据库原tool_arguments，不覆盖Driver参数。
        Existing读取记录parent_node并定位实际父实体；顶层记录父实体为Workspace，子Agent记录要求原父Agent仍存在且可受理，否则拒绝，不改变节点归属。
        构造ToolExecuteRequest::Path，initiator为选定父实体，path和arguments取自变体材料，原reply直接转交；工具登记必须重新检查initiator的ToolTasks受理门。
        同步阶段不准备Agent文件、不打开Agent数据库、不预登记；工具实际执行agent:create时统一检查Driver归属、准备文件、登记及创建实体。
        专用节点名和别名只经arguments进入Driver；ToolExecuteRequest::Path保持initiator/path/arguments/reply四字段，不增加隐式待传材料。

archive_agent_system(world: &mut World)
    处理事件：ArchiveAgent
    接收闭包结果：None
    提交闭包：Future<Output = ()>
    产出事件：None
    行为：同步阶段检查Workspace Running，调用database.try_operation(node_name)取得该节点归档操作凭证；已有创建或归档操作直接返回Busy，不阻塞World。
        从数据库读取记录及其parent_node，普通按node_name、临时按runtime_name定位记录；构造完整目标节点并查找Agent；找到后核对workspace、parent，从Agent.task_id在Agent.parent的ToolTasks取实际任务句柄。匹配Agent存在但父任务缺失是错误，不能当成None。
        无Agent时为None；节点已有在途创建则try_operation已拒绝，不能在此窗口创建新的同节点实体。
        将Option<ToolTaskHandle>、操作凭证、数据库及原reply交给异步闭包；Some先调用finish()，等任务及其绑定Agent、存储和来源监控真实关闭，None跳过。
        blocking worker调用database.archive_agent，在同一受保护操作下记录归档意图、移动目录并提交类别及曾用名；成功后通过原reply回复。
        移动或提交失败返回带阶段的WorkspaceError，保留数据库内归档意图阻止该节点重启；不丢失新旧目录路径，不假报回滚。
        所有分支释放操作凭证；不能恢复一致的归档意图必须在后续显式修复或加载恢复完成后才能继续操作。

```

# handler

## 函数

crate公开：
```text
handle_start_workspace(world: &mut World, event: StartWorkspace)
    行为：用event.address构造并校验Address，检查地址；已占用时先按下述复用/拒绝规则完成，不再建立实体。
        地址空闲时建立实体，直接挂载Address组件。
        用event.alias、event.project_root、event.database构造Workspace，state初始化为Running，并直接挂载到同一实体。
        将tool_core公开的ToolTasks::new()挂载到同一实体。
        组件全部就绪后调用WorkspaceUpdateContext::notify(Installed)，由注入探针发布初始Workspace视图并绑定任务集合及数据库提交通知；event.reply为Some时通过原Sender返回Entity。
        地址已占用时若现有Workspace使用同一管理目录且元数据一致，复用现有实体；仅在same_connection为false时关闭本次独立重复连接，绝不关闭现有共享连接，原reply返回该Entity；否则关闭本次连接并回复错误。加载无reply时记录tracing，不重复挂载。
```

私有：
```text
create_workspace(root: PathBuf, event: CreateWorkspace, events: EventSender) -> Future<Output = ()>
    行为：在异步闭包内通过blocking worker，根据CreateWorkspace创建Workspace目录及数据库文件，写入address、alias、project_root。
        Workspace地址只允许根下单段/name，管理目录为root/name；创建目录必须独占，不覆盖。创建workspace.sqlite和agents/normal、temporary、archived并初始化Schema。
        等待实际写入提交及目录同步完成，构造以空Agent列表为初始changes快照的数据库句柄，通过events直接发送携带address、alias、project_root、database及Some(event.reply)的StartWorkspace。
        目标已存在或写入失败时不覆盖已有Workspace，通过原reply返回错误，不发送启动事件；只回收本次独占创建且尚未对外发布的目录，回收失败在原错误中保留路径。
        不在创建流程建立实体或挂载组件。

load_workspaces(root: PathBuf, events: EventSender) -> Future<Output = ()>
    行为：在异步闭包内通过blocking worker检查root下所有Workspace目录。
        每个Workspace目录使用workspace.sqlite；只读写打开已存在文件，校验schema后recover_archives，再取得地址、别名和项目根构造句柄。
        初始化root/agents/normal、temporary、archived布局只由创建流程建立；加载不对不匹配结构静默补齐。
        初始化数据库句柄时读取一次已提交记录作为changes初始快照；不以SQLite/WAL文件事件冒充数据库提交。
        每发现一个有效Workspace，通过events发送携带address、alias、project_root、database及reply=None的StartWorkspace。
        无效或读取失败的目录记录tracing错误并继续检查其他目录；不创建目录或数据库，不在闭包内建立实体或挂载组件。
```

# types

## 类型

私有：
```text
WorkspaceWorkCompleted：plugin私有Event
    token: Arc<Mutex<Option<WorkspaceWorkToken>>>
    行为：只承载已提交异步工作的World消费确认，不表示Agent或Workspace新增业务状态；克隆事件共享一次take能力。

WorkspaceCleanupResult
    workspace: Entity--完整目标身份
    directory: PathBuf--受理时记录并在结果阶段复核的管理目录
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), WorkspaceError>>>>>
    result: Result<(), WorkspaceError>--清理成功或实际失败，均保留原目标和reply

```

# types pub

## 类型

公开：
```text
WorkspaceUpdateKind
    Installed--组件装配完成，建立一次实际来源观察
    Changed--组件状态或可见字段实际改变
    Closing--失效数据库和任务同步来源
    Removed--实体已销毁，从运行视图移除

WorkspaceRecordsSnapshot：数据库确认提交的Agent登记快照
    revision: u64--进程内来源版本，不是Agent运行状态
    agents: Vec<WorkspaceAgent>--仅已提交的持久记录

WorkspaceState：工作区实体状态，定义于workspace_core
    Running--已启动，可受理操作
    Deleting--已受理删除，正在清理，不接受新操作或重复删除
    Error--删除清理失败，保留实体及资源，不接受新任务，可重试删除
    行为：启动挂载时为Running；删除受理时Running或Error转为Deleting。
        任务清理或数据库关闭失败时Deleting转为Error；成功后销毁实体，不保留已删除状态。

WorkspaceAgent：Workspace数据库中的Agent记录，定义于workspace_core
    parent_node: String--实际创建者的完整节点路径；用于按原层级重启，不是运行Entity
    id: String--记录的唯一标识
    tool_path: PathBuf--实际工具入口文件路径
    tool_arguments: serde_json::Value--宿主捕获的Driver完整调用参数，含其专用参数
    node_name: Option<String>--当前普通节点名；临时及归档记录为None
    alias: Option<String>--当前展示别名
    category: AgentCategory--持久目录类别，不代表实时运行状态
    directory_name: String--类别目录下实际名，不包含分隔符；普通为node_name，临时为记录id
    runtime_name: String--普通为node_name，临时为~temporary-<id>；归档后不可启动
    previous_node_name: Option<String>--归档前曾用的普通节点名
    previous_alias: Option<String>--归档前曾用的别名

AgentCategory：Agent目录类别，定义于workspace_core
    Normal--普通Agent，directory_name为node_name
    Temporary--临时Agent，节点名未提供，directory_name独立分配
    Archived--归档Agent，directory_name在归档时重新分配

WorkspaceAdmission：私有，workspace_core内部
    accepting: bool
    nodes: BTreeSet<String>--正在创建或归档的节点操作键，不是Workspace删除集合
    count: usize
WorkspaceOperation：公开不透明RAII句柄
    key: String
    database_identity: usize--内部共享数据库身份，不能用于网络寻址
    admission: Arc<Mutex<WorkspaceAdmission>>
    idle: Arc<Notify>
    impl Drop
        行为：短锁移除自身key并减count，解锁后Notify唤醒wait_idle；不改持久类别。

WorkspaceDatabase：工作区数据库访问句柄，定义于workspace_core
    path: PathBuf--私有，数据库文件的绝对路径
    connection: Arc<Mutex<Option<rusqlite::Connection>>>--私有，None表示已关闭
    admission: Arc<Mutex<WorkspaceAdmission>>--私有，只管理在途操作，不是运行状态数据库
    idle: Arc<Notify>
    changes: Arc<watch::Sender<Option<WorkspaceRecordsSnapshot>>>--共享确认快照；None表示来源已关闭
    subscribe_changes(&self) -> watch::Receiver<Option<WorkspaceRecordsSnapshot>>
        行为：返回共享提交来源的独立接收端，当前值为最近确认快照；只建立进程内观察，不查询、写入数据库或发送网络消息。
    publish_committed(&self, agents: Vec<WorkspaceAgent>)
        行为：私有，调用方持数据库操作锁，在事务确认提交后提交owned快照；内容相同不推进revision；先更新来源快照再释放操作锁，避免迟到旧快照覆盖新提交。
    path(&self) -> &Path
        行为：返回数据库文件路径。
    same_connection(&self, other: &Self) -> bool
        行为：比较共享connection的Arc身份；同路径的新连接也返回false。
    try_operation(&self, key: &str) -> Result<WorkspaceOperation, WorkspaceError>
        行为：短锁检查受理门及节点操作占用，冲突立即Busy；普通键为Workspace唯一节点名；临时键为~temporary-<新随机id>，同时作为该临时Agent的runtime_name。成功登记操作并返回RAII凭证，不持数据库连接锁跨异步等待。
    close_admission(&self)
        行为：同步关闭新操作受理；已受理凭证继续清理，不立即关闭连接。
    wait_idle(&self) -> Future<Output = ()>
        行为：用Notify等待操作计数归零，先登记等待再检查计数，不轮询、不阻塞World。
    prepare_agent(&self, operation: &WorkspaceOperation, parent_node: String, node_name: Option<String>, alias: Option<String>, tool_path: PathBuf, arguments: serde_json::Value) -> Result<WorkspaceAgent, WorkspaceError>
        行为：读取记录并校验Driver规范路径及parent_node一致；原记录归档或存在归档意图时拒绝，普通记录仅允许同Driver同父复用。
            无记录时分配唯一id并构造候选记录，不提交agents行；临时id由本次临时操作键的随机部分确定，普通目录为node_name，临时目录为id及~temporary-<id>运行名。
            在节点凭证保护下把该候选记录交给注入方法准备文件；新记录的目录若已经存在但无对应数据库记录，返回UnregisteredDirectory，不复用未知归属文件、不自动删除修补。
    commit_agent(&self, operation: &WorkspaceOperation, record: WorkspaceAgent) -> Result<WorkspaceAgent, WorkspaceError>
        行为：确认凭证与目标节点一致，事务重新核对原记录Driver/父节点及id，插入新记录或更新原工具参数和显式alias；并发冲突拒绝，不覆盖其他记录。
            确认提交后publish_committed，相同值不通知；文件准备、StartAgent回复及子Agent绑定都持有同一operation，跨await不持SQL锁。
    agent_by_node(&self, node_name: &str) -> Result<WorkspaceAgent, WorkspaceError>
        行为：优先按普通node_name，保留前缀节点按临时runtime_name返回记录，缺失返回错误。读到归档中原记录不代表它可启动；Existing只允许Normal且ensure_startable通过，临时不重放成另一实例。
    ensure_startable(&self, id: &str) -> Result<(), WorkspaceError>
        行为：要求记录非Archived且无archive_intents，存在意图返回ArchivePending；不改变数据。
    archive_agent(&self, operation: &WorkspaceOperation, id: &str) -> Result<WorkspaceAgent, WorkspaceError>
        行为：同步数据库/文件操作，由plugin在blocking worker调用；operation须属于目标节点，实际运行任务由plugin事先关闭。
            若已有同记录归档意图，按recover_archives的单记录规则继续完成，使用原意图目标目录，不重新分配、不重复移动；正常受理凭证保护下可以再次显式ArchiveAgent恢复。
            无意图时在同一数据库archive_intents表确认写入id、原类别及目录名、新分配归档名；归档名使用新随机值，不复用原名。
            用std::fs::rename将整个目录移入agents/archived，sync父目录；再事务标记Archived、保存previous_node_name及previous_alias、清空当前node_name、更新directory_name并删除意图，确认提交后publish_committed。
            rename失败且确认仍在原位置时清除意图并返回失败；rename成功但提交失败时保留意图、返回ArchivePending，禁止此节点再启动，不假称回滚。
    recover_archives(&self) -> Result<(), WorkspaceError>
        行为：加载数据库时、发布初始快照之前处理archive_intents。原目录存在而目标不存在则完成移动；目标存在而原目录不存在则完成数据库提交；两处同时存在或都不存在则拒绝加载，绝不覆盖猜测。
            仅恢复已持久受理的归档操作，不迁移旧结构、不创建备份。目录检查及移动限制在Workspace管理根，不跟随越界链接。
    close(&self) -> Result<(), WorkspaceError>
        行为：取得连接访问锁并直接关闭连接；所有共享句柄随后拒绝数据库访问。
            受理计数必须为零，先关闭受理，已关闭时成功返回；成功关闭后将changes置为None，使所有持有接收端的监控结束；关闭失败保留连接和来源并返回错误。
    impl Clone for WorkspaceDatabase
        行为：克隆访问句柄，共享同一连接和changes来源，不复制或重新打开数据库。

Workspace数据库结构
    PRAGMA user_version = 1；不匹配明确失败，不自动迁移
    workspace：单行address、alias、project_root
    agents：id主键，parent_node、node_name、alias、category、directory_name、runtime_name、tool_path、tool_arguments、previous_node_name、previous_alias
        node_name非空时在整个Workspace唯一；category只允许Normal/Temporary/Archived，tool_arguments为JSON对象。
        Normal必须有合法node_name且directory_name等于node_name，Temporary无node_name，Archived只保留previous_node_name；所有目录名禁止路径分隔符及父级遍历。
    archive_intents：agent_id主键及外键，source_category、source_directory、destination_directory
        仅表示尚未提交完成的目录移动；不作为实时Agent状态。

```

# error

## 类型

公开：
```text
WorkspaceError：工作区操作错误
    kind: WorkspaceErrorKind
    message: String
    phase: Option<String>--发生失败的实际步骤
    managed_directory: Option<PathBuf>--需要显式修复的已验证管理路径
    entity_removed: bool--删除是否已经移除实体
    impl Clone + Display + std::error::Error
WorkspaceErrorKind：Clone
    InvalidRequest | Missing | Busy | Closed | DriverMismatch | ParentMismatch | DuplicateNode
    UnregisteredDirectory | DatabaseFailed | CloseFailed | ArchivePending | RecoveryFailed | DeleteFailed
```

# 持有关系

```text
WorkspacePlugin
└── root: PathBuf

World
├── WorkspaceConfig（Resource）
│   └── root: PathBuf
└── WorkspaceUpdateContext（Resource）
    └── 组件生命周期回调 → plugin公共视图及来源绑定

Workspace实体
├── Address（address定义）
├── ToolTasks（tool_core定义）
└── Workspace
    ├── alias: String
    ├── project_root: PathBuf
    ├── state: WorkspaceState
    └── database: WorkspaceDatabase
        ├── path: PathBuf
        ├── connection: Arc<Mutex<Option<rusqlite::Connection>>>
        └── changes → 确认提交的登记快照及关闭通知

CreateWorkspace
├── address: String
├── alias: String
├── project_root: PathBuf
└── reply: 原创建请求Sender

StartWorkspace
├── address: String
├── alias: String
├── project_root: PathBuf
├── database: WorkspaceDatabase
└── reply: 可选原创建请求Sender

DeleteWorkspace
├── workspace: Entity
└── reply: Arc<Mutex<Option<oneshot::Sender<Result<(), WorkspaceError>>>>>

StartWorkspaceAgent<R, E>
├── Existing
│   ├── workspace: Entity
│   ├── node_name: String
│   └── reply: Arc<Mutex<Option<oneshot::Sender<Result<R, E>>>>>
└── New
    ├── workspace: Entity
    ├── tool_path: PathBuf
    ├── arguments: serde_json::Value
    └── reply: Arc<Mutex<Option<oneshot::Sender<Result<R, E>>>>>

ArchiveAgent
├── workspace: Entity
├── node_name: String
└── reply: Arc<Mutex<Option<oneshot::Sender<Result<(), WorkspaceError>>>>>

Workspace管理目录
├── Workspace数据库文件
└── agents
    ├── normal
    │   └── <节点名>：普通Agent的完整持久化目录
    ├── temporary
    │   └── <分配名>：临时Agent的独立目录
    └── archived
        └── <归档时新分配名>：整体移入的数据目录，数据库保留曾用节点名和别名
```
