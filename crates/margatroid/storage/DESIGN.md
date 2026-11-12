# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System分别列出事件、闭包结果、提交闭包及oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib        storage/core/src/lib.rs、blocks.rs、blocks/publication.rs、messages.rs、pages.rs、logs.rs；storage/plugin/src/lib.rs  文件、快照和数据库入口；Plugin安装
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
        CreateBlock的TEMP只在DatabaseState.temporary创建内存集合，已存在仅比较子项名，不清空；持久Block调用blocks::create发布工作库及timeline
        ReadMessages/ReadMessageSources固定各来源工作文件视图，读取期间阻止内部写入及回档穿过操作边界
        ReplaceMessages先读取全部来源并验证消息sequence关联；TEMP在受理门内完成范围替换，持久目标在单个写事务内读取、校验范围并替换
        跨文件来源不修改，失败不改目标，不宣称跨文件SQL写事务；普通写入不访问snapshot文件
        DropTemporaryBlocks仅移除temporary中指定VM的Block及内存快照，不读写管理库或文件；其余变体保持历史、日志和设置各自契约
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
        仅提供core诊断读取能力，不自动向LLM暴露；网络范围查询由独立分页函数提供，不改变本函数after升序语义

read_message_page(storage: &AgentStorage, query: RecordRange) -> Result<RecordPage<StoredHistoryRecord>, StorageError>
    行为：在管理连接同一读事务中仅查询agent_messages，按规范列重建完整消息及真实sequence，返回范围内记录及双游标，不合并MCL或日志。

read_mcl_page(storage: &AgentStorage, query: RecordRange) -> Result<RecordPage<StoredHistoryRecord>, StorageError>
    行为：同一只读快照仅查询mcl_records，保留真实source、turn、原指令及审计payload，不读取消息正文表替代审计。

read_agent_log_page(storage: &AgentStorage, query: RecordRange) -> Result<RecordPage<StoredAgentLog>, StorageError>
    行为：同一只读快照仅查询agent_logs，保留级别、来源、阶段、上下文及原始诊断，使用该表独立sequence，不消费日志。

read_record_watermarks(storage: &AgentStorage) -> Result<RecordWatermarks, StorageError>
    行为：在同一管理库读事务取得消息、MCL和Agent日志各表已提交最大sequence，空表为0；只返回事实，不创建记录、不增加本地序号。

snapshot_block(storage: &AgentStorage, name: &str, instance: Option<&str>) -> Result<(), StorageError>
    行为：
        storage_core公开同步函数，阻塞执行到结果确认，不派发事件；异步调用者负责blocking worker及真实join
        None仅访问持久命名空间，Some按实例参数优先TEMP再回退持久；不匹配其他TEMP实例，拒绝文件路径或子项路径
        取得管理锁后按固定顺序取得temporary锁；Some(instance)命中内存Block则深复制其子项集合到本Block局部ID快照并更新上游，不写库或文件；否则解析持久timeline
        分配本Block未使用的新snapshot_id及快照文件名；身份单调分配但不表示当前启用代，不覆盖或复用旧文件
        使用SQLite一致性复制完整工作库，取得所有子项表、消息、顺序及索引，不清空、压缩或重构业务内容
        处理已提交WAL内容，不能fs::copy活动主文件；SQLite复制API只实现显式快照，不额外生成兜底备份
        创建目标副本前通过publication::begin原子发布归属marker；完成副本校验与持久化后在管理库单事务登记snapshot记录，成功移除marker
        snapshot记录复制timeline的block归属、子项映射及原upstream_id，file_name指向新快照
        timeline的file_name不变，仅将upstream_id更新为新snapshot_id；snapshot记录自身不指向自身
        提交后快照永久只读，普通读写继续原工作文件，无须重连、换文件名或携带快照ID
        确认未提交的失败保留工作文件与上游，由publication::abandon回收明确归属的未发布副本；提交事实不明保留marker用于下次核验
        提交事实不明返回CommitUnknown并阻止不确定写入，核实管理库事实，不再次执行请求或重复分配快照
        快照仅操作指定Block工作库与映射，保留显式创建时刻内容；不写入其他存储数据

append_message_history(storage: &AgentStorage, messages: &[serde_json::Value], turn: Option<&str>) -> Result<Vec<serde_json::Value>, StorageError>
    行为：
        取得管理连接锁并检查retired，BEGIN IMMEDIATE后逐条调用messages::append，全部完成才提交
        未带sequence按消息/MCL共享历史MAX分配真实序号；已有sequence校验实际消息记录一致则复用，不再追加
        返回带真实sequence的完整内部消息，错误回滚整个批次，不修改Block或MCL审计

read_block_snapshots(storage: &AgentStorage, name: &str, instance: Option<&str>) -> Result<serde_json::Value, StorageError>
    行为：
        读取指定Block命名空间的timeline和快照映射；命中TEMP从纯内存集合生成，file_name为null；持久从管理库读取
        timeline包含固定file_name、子项映射、当前upstream_id；snapshots包含snapshot_id、file_name、子项映射、upstream_id
        snapshots按本Block的snapshot_id升序排列，根上游为null；不以文件编号大小推断父子关系
        只列本Block，不读取或修改其他Block，不枚举任意目录，不返回未提交的回档中间态
        不执行快照、不消费记录；与快照和回档共用受理门，不派发新事件

restore_block(storage: &AgentStorage, name: &str, instance: Option<&str>, snapshot_id: u64) -> Result<(), StorageError>
    行为：
        对调用者是完整原子操作：回档前归档、恢复目标、提交timeline映射全部收口才返回成功，中间态不对正常读写开放
        验证目标属于同Block同命名空间的已登记只读快照，拒绝跨Block/VM引用及任意路径
        取得存储受理门，命中TEMP则先验证目标、深复制当前内容保存内存快照，再原子替换子项集合并指向目标，完全不落盘；持久路径固定timeline与目标，分配本Block局部ID
        收口工作库读写，完成checkpoint并关闭全部工作连接，确保工作主文件包含已提交WAL内容并妥善处理WAL/SHM
        管理库事务登记restore_intent={archive_id,target_id,target_fields}；旧上游及子项映射留在timeline行，工作/归档/.restore路径由该行与ID规范推导
        intent登记前确认新归档文件不存在；在intent持久化后，将原timeline工作文件改名为新的snapshot文件，确认目录持久化；不复制当前工作内容来保存归档
        新snapshot保留timeline原上游，记录被离开的分支；目标快照本身始终只读不移动
        将目标snapshot完整复制为受控暂存工作库，验证schema和全部子项，持久化后发布到原timeline路径并同步目录
        管理库单事务登记改名产生的snapshot，timeline.upstream_id改为目标ID，子项映射改为目标映射，工作文件名保持原名，清除intent
        释放受理门并返回成功；Block连接每次操作临时打开，后续普通GET/INJECT仍打开同名工作库
        SQLite映射事务和文件改名/复制不是天然跨文件事务；intent负责完成同一次操作，不能仅凭rename原子就宣称整体原子
        intent提交前失败不改变工作内容及上游；提交后直至最终提交不能开放该Block或取消后直接推进普通读写
        崩溃恢复先按intent识别旧工作文件是否已改名，再完成固定目标复制和最终映射提交，不再次分配ID或再次归档
        无法证明文件所处阶段、目标损坏或IO失败不能完成时返回CommitUnknown/CleanupFailed并保持Block封闭，不伪报成功或旧状态未变
        不覆盖任何已登记快照；恢复只能发布到已核验的固定工作路径，回收暂存文件前核对归属
        仅恢复指定Block的数据库及子项映射，不修改其他Block、历史、会话日志或设置


私有，定义于storage_core::blocks：
    initialize(db, root, retired)
        行为：恢复发布marker，完成持久restore_intent，校验全部文件、schema、当前映射及祖先链
    resolve(db, name, instance) -> Mapping
        行为：只解析管理库持久timeline，instance必须为空，校验子项映射并拒绝未完成intent；TEMP在调用方先查内存
    create(db, root, retired, name, fields, instance)
        行为：校验名称、字段及生命周期，建立publication归属，创建Block2子项表并提交timeline；确认未提交失败回收副本
    selected(db, root, memory, selector) / field_values(connection, table) -> Vec<Value>
        行为：TEMP直接读取实例内存向量；持久以只读连接按position读取消息列并join重建，执行数组选择器；写事务复用field_values
    execute(db, root, retired, memory, operation) -> StorageValue
        行为：分派Block操作；替换先读sources，再在目标单事务读取、校验并写入新行；不修改其他库
    finish_restore(db, root, retired, current)
        行为：依据已登记Intent，归档不存在才rename；始终以目标快照重建受控.restore文件，再发布工作文件并最终提交映射
    copy_database(source, destination, fields)
        行为：验证源并以create_new占用目标，通过SQLite Backup逐页复制，关闭目标、校验Block2及持久化
    validate_file(path, fields)
        行为：只读打开，校验user_version=2、精确子项DDL、quick_check及连续position；缺失文件不自动创建
    memory_block_mut(memory, name, instance) -> Option<&mut MemoryBlock>
        行为：校验逻辑标识及非空实例，从(instance,name)键取得TEMP；未命中允许调用者回退持久Block
    locked(storage, operation) / transaction(db, retired, operation)
        行为：先管理连接锁后temporary锁；检查不可用标记；事务使用finish_transaction，提交不明退役，公开函数不外借Connection
    mappings / block_path / filename / directory / hex / valid_name / namespace / validate_fields / field_sql / next_id / snapshot_mapping
        行为：读取映射、校验宿主管理路径和命名空间、生成安全子项DDL、分配Block本地ID并构造归档映射
    sync_file(path) / sync_directory(path)
        行为：确认文件及目录持久化，事实不明返回CommitUnknown
私有，定义于storage_core::blocks::publication：
    begin(root, mapping)
        行为：先create_new写完整.pending并fsync，再rename为publish_*.json并fsync根目录；此后才允许创建数据库副本
    finish(root, mapping) / abandon(root, mapping)
        行为：确认已提交只删除marker；确认未提交删除marker明确归属的数据库及sidecar再删除marker，不触碰其他文件
    recover(db, root)
        行为：清理未发布.pending；完整marker与映射一致则保留文件，未登记则回收其明确归属文件；损坏或归属冲突明确报错
    remove_database(path) / remove_owned_file(path) / marker_path(root, mapping)
        行为：对已由调用方核验归属的文件及-wal/-shm/-journal执行非递归清理，拒绝symlink及非普通文件

私有，定义于storage_core::pages：
    read(storage, operation)
        行为：持管理连接锁，检查retired并开启DEFERRED读事务，执行读取闭包后通过finish_transaction提交或回滚，不外借Connection。
    bounds(query: RecordRange) -> Result<(Option<i64>, Option<i64>, i64), StorageError>
        行为：拒绝零limit、after>=before及超出SQLite整数范围的边界/limit，转换为SQL参数。
    page<T>(db, table, query, decode) -> Result<RecordPage<T>, StorageError>
        行为：在所属表按排他上下界有界选择sequence；无after降序选最新，有after升序选最早，再按sequence升序排列。
            同事务检查本页最小/最大序号之外且原范围内是否有记录，存在才返回对应游标；逐条按真实ID解码，不使用offset或读取整表。

私有，定义于storage_core::messages：
    split(message) -> MessageRow
        行为：严格识别六种type，提取真实sequence、role及内容/data/tool_calls列；其他键仅存payload，拒绝replay/images
    from_row(row, offset) -> MessageRow
        行为：从sequence、role、content、data、tool_call、payload六列读取，不把position当sequence
    join(row, with_sequence) -> Value
        行为：先拒绝payload重复保留字段，再按角色重建内部消息并验证；根据标志恢复真实sequence
    validate_sequence(db, message)
        行为：无sequence只校验消息；有序号必须存在于agent_messages且重建消息去sequence完全一致，否则InvalidRecord
    next_sequence(db) -> i64
        行为：从agent_messages及mcl_records最大序号加1，溢出报错
    append(db, message, turn, timestamp) -> Value
        行为：校验已有序号并幂等返回，未落库消息拆列后追加，返回带真实sequence消息
    encode / take_json
        行为：序列化结构化内容，移出data及tool_calls避免payload重复

数据库结构
    Agent管理库：保留原数据库路径
        main.agent_messages：sequence、turn、role、content、data、tool_call、payload、created_at_ms--消息按统一映射拆列，payload仅剩余字段
        main.mcl_records：sequence、source、turn、content、payload、created_at_ms--纯指令记录
        main.agent_logs：sequence、created_at_ms、level、source、stage、message、details_json、context_json、origin、raw_error、traceback
        main.block_snapshots：record_id、block_name、instance、kind、snapshot_id、file_name、fields_json、upstream_id、restore_intent
            管理库instance约束为空，仅保存持久Block；snapshot_id、文件编号和upstream引用以block_name为作用域，各Block独立分配互不影响；record_id仅内部行标识
            kind为timeline或snapshot；每个逻辑Block恰有一条timeline，创建后不因快照更换该行或工作文件
            timeline内部snapshot_id=0、初始upstream_id=NULL；查询JSON不暴露timeline的内部ID；file_name指固定工作库，fields_json保存子项到表的映射
            snapshot每条记录对应一张不可变副本，snapshot_id标识它，upstream_id记录创建时timeline的原上游
            upstream_id同Block同命名空间引用另一条snapshot，根为空；不指向timeline、不形成自环
            普通快照：新记录upstream=timeline旧上游，timeline上游=新记录；timeline工作文件保持不动
            回档：先记intent，将工作文件改名归档，复制目标恢复原工作路径；最终同事务登记归档及timeline目标上游
            fields_json仅保存子项名到物理表名，不保存消息数组；恢复时与目标文件同步切换
            restore_intent仅timeline允许非空，保存持久回档状态，不新增管理事件或额外备份机制
    Block文件：<管理库完整路径>.blocks/<Block逻辑名>/timeline.sqlite及snapshot_<id>.sqlite
        一个Block固定一个可写timeline数据库；每次快照从当前工作库完整复制一个只读数据库，不轮换工作文件
        一张子项表对应一个Block子项，每条消息为position INTEGER PRIMARY KEY、sequence可空、role、content、data可空、tool_call可空、payload
        role保存内部type；system/user/tool_call结果content为原文本；assistant content为{reasoning,content}JSON；inject content为messages数组JSON；error message映射content
        data及tool_calls分别存JSON到data与tool_call列，其他字段进payload；sequence仅引用真实历史，不新建Block局部序号；读取重建完整内部消息
        快照及恢复均按完整数据库文件操作，所有子项表与库内状态自然一起保留，不逐子项重建或按当前定义补表
        position表达当前数组顺序，不充当跨快照稳定消息ID；负索引和范围仍按既有数组语义计算
        物理目录、文件名及表名由宿主规范生成，不以未验证业务名拼接SQL或任意路径；空子项也建空表
        普通快照、回档前快照及目标快照均不被后续普通写入修改，不因切换分支自动删除
    TEMP：DatabaseState.temporary纯内存集合
        (instance,Block逻辑名)映射MemoryBlock，各子项为消息向量，快照深复制整个子项集合并保留局部ID和上游
        TEMP内容、映射、快照和恢复状态不进入管理库，不创建目录、SQLite文件或intent；丢弃存储句柄即释放全部内存
        DropTemporaryBlocks在真实VM及MCL退出后只移除该实例数据，不影响其他实例或持久Block
    行为：
        PRAGMA user_version管理库=5、Block文件=2；旧布局明确拒绝，不迁移
        空目录创建新布局；旧main.blocks布局、缺失文件、schema或映射不一致明确报错，不迁移、不备份、不兼容
        初始化核对timeline、snapshot及其上游，先完成明确restore_intent，才允许一般读写；不猜测生成丢失快照
        初始化先publication::recover：按完整归属marker核验已提交/未提交文件，未发布.pending只删除自身；不碰无marker无映射的文件
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
DatabaseState：私有，定义于storage/core/src/types.rs
    connection: Mutex<Connection>--管理库连接同时作为Block串行受理门
    retired: AtomicBool
    block_root: PathBuf--管理库完整路径追加.blocks
    temporary: Mutex<BTreeMap<(String,String),MemoryBlock>>--只保存实例内存内容与快照
Mapping：私有，定义于storage/core/src/blocks.rs
    name: String
    instance: String--持久为空串
    id: i64--timeline=0、snapshot>0
    file: String--受控相对路径
    fields: BTreeMap<String, String>--子项到fN
    upstream: Option<i64>
    intent: Option<Intent>
Intent：私有，定义于storage/core/src/blocks.rs
    archive_id: i64
    target_id: i64
    target_fields: BTreeMap<String, String>

MemoryBlock：私有，定义于storage/core/src/blocks.rs
    fields: BTreeMap<String,Vec<Value>>
    upstream: Option<u64>
    snapshots: BTreeMap<u64,MemorySnapshot>
    new(fields) -> Result<Self,StorageError>--校验唯一非空子项
    snapshot() -> Result<(),StorageError>--本Block分配ID并深复制全部子项
    restore(id) -> Result<(),StorageError>--目标存在才保存当前快照并替换全部子项
    query() -> Value--timeline/snapshot file_name均为null
MemorySnapshot：私有，定义于storage/core/src/blocks.rs
    fields: BTreeMap<String,Vec<Value>>
    upstream: Option<u64>
MessageRow：私有，定义于storage/core/src/messages.rs
    sequence: Option<i64>
    role: String
    content: Option<String>
    data: Option<String>
    tool_call: Option<String>
    payload: String

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
    DropTemporaryBlocks { instance: String }--沿原事件与变体释放该VM实例TEMP内存Block和快照；不动持久归档
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
    content: String--MCL原文独立保存；消息写入按payload的统一映射提取，读取对应存储content
    payload: serde_json::Value--对外完整重建消息带sequence，SQL payload仅保存剩余字段
    created_at_ms: Option<u64>--新记录缺省时取真实写入时间

RecordRange：记录范围，公开纯值类型
    before_sequence: Option<u64>--排他上界
    after_sequence: Option<u64>--排他下界
    limit: usize--非零；双界同时存在时after必须小于before
    行为：无after时选上界之前最新limit条；有after时选下界之后最早limit条并受上界约束，结果始终sequence升序，不使用offset。
RecordPage<T>：有界读取结果，公开纯值类型
    records: Vec<T>
    next_before_sequence: Option<u64>--当前范围内仍有更早记录时为本页最小sequence
    next_after_sequence: Option<u64>--当前范围内仍有更新记录时为本页最大sequence
    行为：记录与游标由同一读事务确定；无对应方向剩余记录为None，空页不是查询失败。
RecordWatermarks：独立持久记录水位
    message: u64
    mcl: u64
    agent_log: u64

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
    │   └── 按VM实例隔离的TEMP内存Block及快照
    │       └── 生命周期结束后释放该实例内存，不接触磁盘
    ├── settings_path
    └── settings_lock

mecs全局异步设施
└── storage_task
    ├── owned请求及reply
    └── blocking worker
        └── SQLite事务或TOML文件操作
```
