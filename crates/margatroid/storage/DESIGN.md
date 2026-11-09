# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System分别列出事件、闭包结果、提交闭包及oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib        storage/core/src/lib.rs；storage/plugin/src/lib.rs  文件和数据库入口；Plugin安装
# components storage/core/src/components.rs                     AgentStorage
# events     storage/core/src/events.rs                         存储请求
# system     storage/plugin/src/system.rs                       请求和闭包结果路由
# handler    storage/plugin/src/handler.rs                      句柄取得、worker提交与回复
# types      storage/plugin/src/types.rs                        异步上下文和输出
# types pub  storage/core/src/types.rs                          Block、设置、历史及Agent日志类型
# error      storage/core/src/error.rs                          StorageError
```

# lib

## 函数

公开，定义于storage_core：
```text
open_storage(database_path: &Path, settings_path: &Path, default_settings: &str) -> Result<AgentStorage, StorageError>
    行为：打开Agent管理库及Block工作文件及快照管理器，验证管理库和已登记Block文件的目标schema与映射；settings不存在时以不覆盖方式写入默认模板，存在则读取并校验；返回共享句柄，不缓存消息正文。

execute_storage(storage: &AgentStorage, operation: StorageOperation) -> Result<StorageValue, StorageError>
    行为：
        按StorageOperation分派；Block操作通过共享受理门解析timeline，管理库操作使用管理库事务，设置操作使用设置文件锁
        CreateBlock不存在时创建工作库和子项表并登记唯一timeline；已存在且字段及生命周期一致则打开，不清空
        ReadMessages/ReadMessageSources固定各来源工作文件视图，读取期间阻止内部写入及回档穿过操作边界
        ReplaceMessages先读取全部来源，再在目标Block文件单个事务中读取目标、校验范围及替换，自引用先读后写
        跨文件来源不修改，失败不改目标，不宣称跨文件SQL写事务；普通写入不访问snapshot文件
        DropTemporaryBlocks仅清理指定VM命名空间的映射及文件；其余变体保持历史、日志和设置各自契约
        等待写入及必要清理确认后返回，不让旧工作连接绕过回档受理门；raw Connection不外借。

append_agent_log(storage: &AgentStorage, record: AgentLogRecord) -> Result<StoredAgentLog, StorageError>
    行为：
        同步执行独立日志写事务，与execute_storage共用连接管理器和串行锁
        校验level、source及JSON对象，缺省created_at_ms取真实写入时间
        以agent_logs自身MAX(sequence)+1分配序号，空表从1开始；超过SQLite整数范围返回错误
        SQLite INTEGER PRIMARY KEY不使用AUTOINCREMENT；提交成功后返回已落库记录
        不写历史、不触发推理，不派发StorageRequest或System事件
        异步调用方克隆AgentStorage，在spawn_blocking中调用并等待真实join，不将排队视为成功

read_agent_logs(storage: &AgentStorage, after_sequence: Option<u64>, limit: usize) -> Result<Vec<StoredAgentLog>, StorageError>
    行为：
        同步取得独立日志读事务快照，与其他同库操作共用连接及串行锁
        None从首条读取，Some仅取sequence严格大于给定值的记录
        按sequence升序至多返回limit条，limit为0返回空数组
        不合并历史、不派发事件；异步调用方使用spawn_blocking并等待真实join
        仅提供core诊断读取能力，不自动向LLM暴露，不新增DTO或GUI接口

snapshot_block(storage: &AgentStorage, name: &str, instance: Option<&str>) -> Result<(), StorageError>
    行为：
        storage_core公开同步函数，阻塞执行到结果确认，不派发事件；异步调用者负责blocking worker及真实join
        None访问持久命名空间，Some仅访问绑定VM的TEMP；拒绝文件路径、子项路径和其他实例
        取得共享受理门并等待该Block在途读写收口，读取其唯一timeline记录及upstream_id
        分配本Block未使用的新snapshot_id及快照文件名；身份单调分配但不表示当前启用代，不覆盖或复用旧文件
        使用SQLite一致性复制完整工作库，取得所有子项表、消息、顺序及索引，不清空、压缩或重构业务内容
        处理已提交WAL内容，不能fs::copy活动主文件；SQLite复制API只实现显式快照，不额外生成兜底备份
        完成副本校验与文件、目录持久化，再在管理库单个事务登记snapshot记录
        snapshot记录复制timeline的block归属、子项映射及原upstream_id，file_name指向新快照
        timeline的file_name不变，仅将upstream_id更新为新snapshot_id；snapshot记录自身不指向自身
        提交后快照永久只读，普通读写继续原工作文件，无须重连、换文件名或携带快照ID
        提交前失败工作文件和timeline上游不变；只回收可证明由本次创建且未发布的副本，不删除已登记快照
        提交事实不明返回CommitUnknown并阻止不确定写入，核实管理库事实，不再次执行请求或重复分配快照
        快照仅操作指定Block工作库与映射，保留显式创建时刻内容；不写入其他存储数据

read_block_snapshots(storage: &AgentStorage, name: &str, instance: Option<&str>) -> Result<serde_json::Value, StorageError>
    行为：
        读取指定Block命名空间的timeline和全部已提交snapshot映射，返回结构化对象
        timeline包含固定file_name、子项映射、当前upstream_id；snapshots包含snapshot_id、file_name、子项映射、upstream_id
        snapshots按本Block的snapshot_id升序排列，根上游为null；不以文件编号大小推断父子关系
        只列本Block，不读取或修改其他Block，不枚举任意目录，不返回未提交的回档中间态
        不执行快照、不消费记录；与快照和回档共用受理门，不派发新事件

restore_block(storage: &AgentStorage, name: &str, instance: Option<&str>, snapshot_id: u64) -> Result<(), StorageError>
    行为：
        对调用者是完整原子操作：回档前归档、恢复目标、提交timeline映射全部收口才返回成功，中间态不对正常读写开放
        验证目标属于同Block同命名空间的已登记只读快照，拒绝跨Block/VM引用及任意路径
        取得存储受理门，固定timeline旧上游、子项映射及目标快照；分配仅属于该Block的新snapshot_id
        收口工作库读写，完成checkpoint并关闭全部工作连接，确保工作主文件包含已提交WAL内容并妥善处理WAL/SHM
        管理库事务登记持久restore_intent，包含原工作路径、新归档路径及ID、旧上游与子项映射、目标快照与受控暂存路径
        在intent持久化后，将原timeline工作文件改名为新的snapshot文件，确认目录持久化；不复制当前工作内容来保存归档
        新snapshot保留timeline原上游，记录被离开的分支；目标快照本身始终只读不移动
        将目标snapshot完整复制为受控暂存工作库，验证schema和全部子项，持久化后发布到原timeline路径并同步目录
        管理库单事务登记改名产生的snapshot，timeline.upstream_id改为目标ID，子项映射改为目标映射，工作文件名保持原名，清除intent
        重新建立工作连接，释放受理门并返回成功；普通GET/INJECT用法不变
        SQLite映射事务和文件改名/复制不是天然跨文件事务；intent负责完成同一次操作，不能仅凭rename原子就宣称整体原子
        intent提交前失败不改变工作内容及上游；提交后直至最终提交不能开放该Block或取消后直接推进普通读写
        崩溃恢复先按intent识别旧工作文件是否已改名，再完成固定目标复制和最终映射提交，不再次分配ID或再次归档
        无法证明文件所处阶段、目标损坏或IO失败不能完成时返回CommitUnknown/CleanupFailed并保持Block封闭，不伪报成功或旧状态未变
        不覆盖任何已登记快照；恢复只能发布到已核验的固定工作路径，回收暂存文件前核对归属
        仅恢复指定Block的数据库及子项映射，不修改其他Block、历史、会话日志或设置

数据库结构
    Agent管理库：保留原数据库路径
        main.agent_messages：sequence、turn、type、content、payload、created_at_ms--纯消息历史
        main.mcl_records：sequence、source、turn、content、payload、created_at_ms--纯指令记录
        main.agent_logs：sequence、created_at_ms、level、source、stage、message、details_json、context_json、origin、raw_error、traceback
        main.block_snapshots：record_id、block_name、instance、kind、snapshot_id、file_name、fields_json、upstream_id、restore_intent
            snapshot_id、文件编号和upstream引用均以block_name及instance为作用域，各Block独立分配互不影响；record_id仅内部行标识
            kind为timeline或snapshot；每个逻辑Block恰有一条timeline，创建后不因快照更换该行或工作文件
            timeline初始snapshot_id及upstream_id为空；file_name指向固定工作库，fields_json记录子项到表的映射
            snapshot每条记录对应一张不可变副本，snapshot_id标识它，upstream_id记录创建时timeline的原上游
            upstream_id同Block同命名空间引用另一条snapshot，根为空；不指向timeline、不形成自环
            普通快照：新记录upstream=timeline旧上游，timeline上游=新记录；timeline工作文件保持不动
            回档：先记intent，将工作文件改名归档，复制目标恢复原工作路径；最终同事务登记归档及timeline目标上游
            fields_json仅保存子项名到物理表名，不保存消息数组；恢复时与目标文件同步切换
            restore_intent仅timeline允许非空，保存持久回档计划，不新增管理事件或额外备份机制
    Block文件：blocks/<宿主生成的Block标识>/session.sqlite及session_<snapshot_id>.sqlite--session为示意
        一个Block固定一个可写timeline数据库；每次快照从当前工作库完整复制一个只读数据库，不轮换工作文件
        一张子项表对应一个Block子项，每条消息为position INTEGER PRIMARY KEY、payload TEXT NOT NULL
        快照及恢复均按完整数据库文件操作，所有子项表与库内状态自然一起保留，不逐子项重建或按当前定义补表
        position表达当前数组顺序，不充当跨快照稳定消息ID；负索引和范围仍按既有数组语义计算
        物理目录、文件名及表名由宿主规范生成，不以未验证业务名拼接SQL或任意路径；空子项也建空表
        普通快照、回档前快照及目标快照均不被后续普通写入修改，不因切换分支自动删除
    TEMP文件：VM实例隔离的运行时临时目录
        同样一个固定timeline工作文件及若干只读快照；本实例全部文件寿命仍属于该VM，不变成永久归档
        VM、MCL及worker真实退出后沿DropTemporaryBlocks清理对应映射及本实例全部工作文件/快照
        不清理持久快照目录；异常退出遗留临时目录核验后清理，不恢复成持久Block
    行为：
        PRAGMA user_version标记管理库及Block文件目标版本，不冒用现有version 3四表schema
        空目录创建新布局；旧main.blocks布局、缺失文件、schema或映射不一致明确报错，不迁移、不备份、不兼容
        初始化核对timeline、snapshot及其上游，先完成明确restore_intent，才允许一般读写；不猜测生成丢失快照
        未登记副本只在可证明属于未完成发布的内部文件时回收，不触碰已登记快照或外部文件
        不承诺多个Block文件的统一SQL事务，Block内容和管理库中的历史/日志仍是独立提交

```

## 类型

公开，定义于storage_plugin：
```text
StoragePlugin：存储执行插件
    schedule: String
    impl Plugin for StoragePlugin
        build(self, app: &mut App)
            行为：检查异步设施，拒绝重复安装；安装storage_system。
```

# components

## 类型

公开：
```text
AgentStorage：Agent持久存储，公开组件
    database: StorageDatabase--私有，共享管理库、Block映射及当前文件连接能力
    settings_path: PathBuf--私有，当前Agent设置文件
    settings_lock: Arc<Mutex<()>>--私有，协调该文件读写
    impl Component for AgentStorage
    impl Clone for AgentStorage
        行为：共享连接及设置锁，不复制数据；Driver与工具的host持有owned克隆作为日志能力，仅依赖storage_core；生命周期覆盖Driver及工具清理和最后一次日志写入的真实join，再释放句柄。
```

# events

## 类型

公开：
```text
StorageRequest：存储操作，公开事件
    owner: Entity
    operation: StorageOperation
    cancel: CancellationToken
    reply: Arc<Mutex<Option<oneshot::Sender<Result<StorageValue, StorageError>>>>>
    impl Event for StorageRequest
```

# system

## 函数

crate公开：
```text
storage_system(world: &mut World)
    处理事件：StorageRequest
    接收闭包结果：Result<StorageOutput, StorageTaskError>
    提交闭包：storage_task(StorageContext) -> Future<Output = Result<StorageOutput, StorageTaskError>>
    产出事件：None
    oneshot回复：Result<StorageValue, StorageError>
    行为：StorageRequest -> handle_storage_request；Result<StorageOutput, StorageTaskError> -> handle_storage_result。
```

# handler

## 函数

crate公开：
```text
handle_storage_request(world: &mut World, request: StorageRequest)
    行为：认领reply并核对完整owner；克隆AgentStorage及参数，提交storage_task；提交失败回复原请求。

handle_storage_result(world: &mut World, result: Result<StorageOutput, StorageTaskError>)
    行为：核对原owner，等待worker真实join；向原reply交付真实执行结果；不把已提交写入改报为取消，不自动重试。
```

私有：
```text
storage_task(context: StorageContext) -> Future<Output = Result<StorageOutput, StorageTaskError>>
    行为：spawn_blocking执行execute_storage并等待真实join；保留原reply及提交事实，错误转换为StorageTaskError。
```

# types

## 类型

私有：
```text
StorageContext：执行上下文--owner、AgentStorage、StorageOperation、cancel、原reply
StorageOutput：执行成功载荷--owner、原reply、StorageValue
StorageTaskError：执行失败载荷--owner、原reply、StorageError
```

# types pub

## 类型

公开：
```text
StorageDatabase：不透明SQLite连接能力
    impl Clone
        行为：共享管理库连接、Block工作文件及快照管理器、串行受理门及不可用标记，不公开Connection，不让调用方绕过timeline及回档受理门。

AgentLogLevel：日志级别，公开枚举
    Debug | Info | Warn | Error--数据库分别保存debug、info、warn、error

AgentLogSource：可信运行身份，公开枚举
    Driver | Tool | Host--数据库分别保存driver、tool、host

AgentLogRecord：独立Agent日志写入值，公开类型
    created_at_ms: Option<u64>--宿主时间；None由写事务填入真实写入时间
    level: AgentLogLevel
    source: AgentLogSource--宿主按原始可信运行身份填入，跨host转发不改成host
    stage: String--描述性阶段，如load、configure、execute、driver、hostlog，不承担操作路由
    message: String
    details_json: Option<serde_json::Value>--可选JSON对象，调用方结构化详情
    context_json: Option<serde_json::Value>--可选宿主关联JSON对象；turn、task、call、alias、driver_instance均可缺省，不另建关联列或标识注册表
    origin: Option<String>--宿主确定的程序来源地址，不接受脚本指定来源
    raw_error: Option<String>--自动故障格式化前的完整原始错误
    traceback: Option<String>--自动故障格式化前的完整Lua traceback，可用时保存

StoredAgentLog：持久日志返回值，公开类型
    sequence: u64--agent_logs独立序号，不占用消息与MCL历史sequence
    record: AgentLogRecord--成功写入或读取时created_at_ms为Some真实存储值

StorageOperation：机械存储操作，公开枚举
    CreateBlock { name: String, fields: Vec<String>, instance: Option<String> }
    ReadMessages { selectors: Vec<MessageSelector> }
    ReadMessageSources { sources: Vec<MessageSource> }
    DropTemporaryBlocks { instance: String }--沿原事件与变体清理该VM实例TEMP映射和全部工作/快照文件；不动持久归档
    ReplaceMessages { sources: Vec<MessageSource>, target: MessageSelector }
    ReadConfig
    SetConfig { path: String, value: toml::Value }
    DeleteConfig { path: String }
    AppendHistory { records: Vec<HistoryRecord> }
    ReadHistory
    ReadHistoryEntries
    ReadAgentMessages--仅AgentMessage表
    ReadMclRecords--仅MCL表

MessageSelector：消息选择器
    block: String
    field: String
    instance: Option<String>--None为持久Block，Some为本次VM临时命名空间
    selection: MessageSelection

MessageSelection：选择范围，公开枚举
    All | Index(i64) | Range(i64, i64)

MessageSource：固定工作文件视图后读取的消息来源，公开枚举
    Stored(MessageSelector)
    Literal(Vec<serde_json::Value>)

HistoryRecord：只追加历史记录
    source: Option<String>--仅MCL必需真实来源地址；消息为None，不保存Entity编号
    turn: Option<String>
    kind: String--时间线记录类型，决定写入message或mcl表，不存为混合kind列
    content: String--调用方提供的原始正文，独立于payload原样保存与读取
    payload: serde_json::Value
    created_at_ms: Option<u64>--新记录缺省时取真实写入时间

StorageValue：存储成功值，公开枚举
    Unit
    Messages(Vec<serde_json::Value>)
    Config(toml::Table)
    History(Vec<HistoryRecord>)
    HistoryEntries(Vec<StoredHistoryRecord>)

StoredHistoryRecord：持久记录读取值
    sequence: u64--Agent内跨消息与MCL表的稳定顺序及记录标识
    record: HistoryRecord
```

# error

## 类型

公开：
```text
StorageError：存储错误，公开类型
    kind: StorageErrorKind
    message: String
    impl Display + std::error::Error

StorageErrorKind：错误类别，公开枚举
    OwnerMissing | Closed | InvalidRequest | InvalidRecord
    SchemaMismatch | LifecycleMismatch | BlockMissing | FieldMissing | InvalidRange
    ConfigInvalid | ConfigPathInvalid | ConfigTypeMismatch
    ReadFailed | WriteFailed | Cancelled | CommitUnknown | CleanupFailed | WorkerFailed
```

# 持有关系

```text
Agent Entity
└── AgentStorage
    ├── StorageDatabase
    │   ├── 共享串行受理门及不可用标记
    │   ├── Agent管理库连接
    │   │   ├── main.agent_messages--纯消息历史
    │   │   ├── main.mcl_records--纯MCL审计
    │   │   ├── main.agent_logs--独立会话日志
    │   │   └── main.block_snapshots--timeline/snapshot文件、子项映射、上游与在途回档
    │   ├── 持久Block文件
    │   │   ├── 每Block固定timeline工作库--普通快照不切换
    │   │   └── 不可变snapshot文件--每子项一表，每消息一行
    │   └── 按VM实例隔离的TEMP工作文件及快照
    │       └── 生命周期结束后核验清理本实例全部文件，不永久归档
    ├── settings_path
    └── settings_lock

mecs全局异步设施
└── storage_task
    ├── owned请求及reply
    └── blocking worker
        └── SQLite事务或TOML文件操作
```
