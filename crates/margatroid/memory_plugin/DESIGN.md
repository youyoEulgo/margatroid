# 板块格式

正文保留lib、system、handler、events、types、error六板块。**已实现一个Agent SQLite backend：Block持久化与history共享connection manager，CREATE/GET/INJECT通过共享异步storage入口调用blocking worker。** 本模块保留历史兼容API，不提供RAM Block或自动同步。Driver/Other登记及资源解析由调用方负责；本模块只报告真实事务与清理事实。

恢复已完成；存储迁移与领域路由已接通。下述行为按当前实现描述，验证范围不扩张为所有故障的保证。

# lib

## 边界

## 集中工作区数据库路径（目标与当前边界）

数据库为 ~/.margatroid/workspaces/<workspace>/agents/<agent>/state.sqlite，同一文件保留现有Block、会话和history结构。路径由组合根注入的工作区存储服务推导，不由项目目录或YAML自定义history_path决定；工作区调用方负责传入受检绝对路径；底层open仍接受AsRef<Path>（独立调用/测试可使用:memory:），不自行强制目录布局、不扫描工作区或选择成员。home_root不是数据库路径。

移除Agent默认保留该文件，同名再加入恢复旧数据；project_root改动不迁库。物理迁移在所有相关连接关闭且WAL正确收口后执行，验证前保留旧数据；不存在跨YAML、ECS与SQLite的原子事务。下文AgentStartDefinition.history_path是当前字段名，不表示继续允许用户配置任意历史路径；共享启动材料继续以此内部字段传入系统推导的路径。

名称派生身份与重命名（已实现，验证范围见下文）：Workspace/Agent名称参与ResourceID，不新增另一套稳定身份。Workspace重命名只在工作区停止且所有相关数据库句柄关闭后进行；运行中成员重命名须先关闭受理、排空/停止该成员并关闭全部句柄，再迁移。迁移范围必须枚举：已知类型化Block namespace（含生命周期定义及需保持一致的实例元数据）和当前history.query实际使用的身份/查询元数据；工具/沙箱ID仅在属于明确身份映射的类型化字段时处理，不重命名无关资源。当前history.query先解析活动Agent，再克隆history句柄并以AgentStateDto的workspace/agent标注记录；重命名须将当前查询身份、句柄及投影元数据重新绑定到新成员，而不是改写历史payload.target或原始来源。保留原历史正文、原始MCL文本/参数、来源、工具调用事实、顺序及真实时间，不盲目字符串替换JSON/正文/审计。目录、YAML、SQLite是独立提交；须预检目标冲突、备份/恢复路径、WAL收口并验证迁移后可打开原会话；任一步失败保留可恢复数据并如实报告已完成/未完成/未知的步骤，不声称跨store原子性。

持久迁移由 Compose 的 `FileWorkspaceCatalogStore` 内部操作实现负责 journal、staging、backup、提交和恢复，调用 Memory 的 `prepare_agent_identity_copy` 制作一致 SQLite 副本并迁移精确 Block agent namespace。Workspace 负责完整 lease、真实停止和句柄关闭；共享 types 不提供独立的 WorkspaceMigrationService/Plan/Progress API。原始 history 全列保持不变，新运行身份和 history 句柄负责当前查询关联。文件存储构造及后续受理检查执行内部 recovery；`.operations`、`.trash` 与内部暂存项不参与 catalog 枚举。journal 使用 `preparing`、`prepared`、`committed`、`complete`、`rolled_back` 阶段：未提交且源校验通过时回滚准备阶段，已提交时幂等完成安装。未知、损坏或无法安全恢复的 journal 会设置存储级 `recovery_required`，保守阻止所有 catalog-backed 加载、启动和变更，而非仅封锁源/目标；诊断目录仍可列出。不能静默打开部分新身份或按任意 journal 路径写入。已有阶段中断及重复恢复测试不等于穷举磁盘故障。

SQLite身份迁移的当前schema算法（已实现）：

```text
prepare_agent_identity_copy(source, staged_destination, old_agent, new_agent)
    要求旧Agent所有database/history/block句柄已关闭，源/目标均为经受检目录服务确定的路径
    用SQLite backup生成完整一致副本，不复制仍活跃的主文件冒充一致备份
    校验已知history_records/block_definitions/block_fields列、主键、外键与schema
    副本中BEGIN IMMEDIATE，预检new_agent命名空间是否已存在；存在则冲突拒绝，不覆盖/合并
    UPDATE block_definitions SET agent=new_agent WHERE agent=old_agent
    保留definition id、name、instance、temporary、schema_json及block_fields原值；字段外键无需重建
    原TEMP行的instance保持原样，新Driver实例仍隔离，不能因改名复活旧entry
    当前StoredBlockInner只保存Message或Tool/Sandbox资源ID；不改无关资源地址，不替换文本/任意JSON
    history_records的sequence/key/source/turn/kind/content/payload/created_at_ms全部保留
    COMMIT并检查integrity_check/foreign_key_check，确认Persistent Block以new_agent可寻址
    验证源与副本历史逐列完全相同、block字段完整、只有明确的namespace列映射变化
    返回 Result<(), String>，校验失败明确错误；不在此安装正式目录或改YAML
```

历史查询无需向旧记录打新来源标签：重建新身份Agent及其history句柄，协议用新的当前AgentStateDto.workspace/agent归属投影原记录。旧审计source/turn/key继续是事实，不作为当前选择身份。数据库中发现未知的身份schema、目标冲突或无法证明引用范围时停止迁移并保留源，不以全库字符串替换“修复”。原库和副本安装进展由Compose存储内部操作及.operations日志管理；schema支持范围和每阶段故障注入是恢复前提。

memory_plugin提供一个真实SQLite backend，不提供RAM Block backend。一个Agent的Block与history共用同一SQLite文件、connection manager并分表存放。沿用AgentStartDefinition.history_path作为数据库路径，不在本轮引入启动参数改名。依赖共享types契约，不依赖agent_plugin或MCL解析实现，不负责消息领域转换、alias注册、资源加载、沙箱激活或Driver生命周期。

Agent只保存AgentDatabaseHandle及同文件AgentHistoryHandle视图，不保存AgentBlock/BlockAssembly内存内容。SQL及连接管理留在本模块，opaque共享句柄不暴露Connection。历史是只追加领域；Block是显式CREATE/GET/INJECT管理的数据，不能用history恢复拼接Block，也不重建旧state表接口。

历史created_at_ms持久保存真实Unix毫秒时间；未指定时间的新记录在事务写入时取得当前时间，幂等重试不覆盖已存时间。旧history_records表缺少时间列时通过事务迁移增加列，旧行保留0表示不可恢复的未知时间；不得以迁移时间、文件修改时间或sequence推测旧时间。协议直接投影已存值，历史顺序仍以sequence为准。

## 回合中止消息的存储边界（已实现）

取消assistant仍属于显式message历史：content保存实际累计正文，无输出保持空字符串，不自动填入文案，payload保留`type: assistant`与`stop_reason: cancelled`及其余元数据，不重复正文。工具取消结果保留调用ID、名称、resource_id与failed，正文保存英文取消说明。历史追加仍由Driver显式history_append执行；存储层不因取消自动创建消息，不根据正文判断取消，也不删除已发生的历史。取消assistant不进模型上下文是Driver规则，不是SQLite暗中拒绝该消息；工具结果则正常进入历史和上下文。Block与历史是不同事务，仍不承诺跨操作原子性，取消不撤销已提交的数据库写入。

## 类型

```text
SqliteAgentDatabase：已实现SQLite backend，Clone + Send + Sync
    open(path: impl AsRef<Path>) -> Result<Self, BlockStorageError>
        同步创建父目录、打开数据库、设置5秒忙等待并open/validate历史与Block表；调用方必须在blocking worker中调用，不清空已有数据。
    impl AgentHistoryStore
        append_batch(Vec<HistoryRecord>) -> Result<(), HistoryError>
        list() -> Result<Vec<HistoryRecord>, HistoryError>
    impl AgentBlockStorage
        execute(BlockStorageRequest) -> Result<StoredBlockResult, BlockStorageError>
    说明：AgentDatabaseStore由共享trait blanket实现；AgentDatabaseHandle::new(store)建立opaque能力。克隆共享Arc<Mutex<Connection>>及retired标记，不另为Block建立第二个数据库文件。

SqliteHistoryStore：现有历史实现，Clone + Send + Sync
    open(path: impl AsRef<Path>) -> Result<Self, HistoryError>
        兼容API：创建父目录、打开数据库、设置5秒忙等待并仅初始化历史表；Agent统一启动使用SqliteAgentDatabase，再由数据库句柄取得共享连接的历史视图。
    impl AgentHistoryStore
        append_batch(Vec<HistoryRecord>) -> Result<(), HistoryError>
        list() -> Result<Vec<HistoryRecord>, HistoryError>

MemoryPlugin：Default，可选资源标记安装
    new() -> Self
    impl Plugin
        build() 插入MemoryPluginInstalled，重复安装报错；不注册system、event或schedule。
```

# system

## 逻辑

无业务System，不轮询数据库、不做RAM到SQLite同步、不隐式记录消息，不通过转发System写历史。ECS/MCL主线程不得执行SQLite IO；存储请求由async handle调度blocking worker，返回关联结果供原请求队列收口。启动open及恢复检查同样不阻塞ECS。

# handler

## 逻辑

```text
打开与生命周期：
    数据库open/validate保留旧表与记录，不迁移删除既有无关表。
    Persistent namespace = 名称派生Agent ResourceID（名称不变时稳定） + block名，重启继续open。
    Temporary namespace = Agent ResourceID + 完整Driver instance + block名；instance编码Entity index/generation与start_request_id。
    启动方必须提供跨进程唯一的start_request_id（包含startup/session nonce），否则相同instance值仍指向同一TEMP定义；数据库不会为每次open另造nonce。
    CREATE BLOCK默认持久；CREATE TEMP BLOCK也是真实SQLite数据，不是SQLite TEMP connection表或RAM假backend。
    CREATE首次才返回BlockOpened.created=true；已有同schema返回false、不清空，schema不符失败且保持原数据。
    同instance重复TEMP CREATE也幂等open/validate；同Agent/block名的persistent/temporary lifecycle由definitions全局保留，跨instance切换也拒绝。
    旧TEMP物理行不自动删除且不同新instance不可见；它仍保留名称的temporary生命周期，不能静默改为persistent。未来显式清理/迁移不在当前API内。
    可见定义完全由SQLite metadata查询，不另存RAM definition映射或Block缓存。
    TEMP entry不恢复；启动不把旧TEMP当作本instance，不恢复turn、pending、alias/import或activeSandboxValue。
    启动IMPORT重建RAM bindings；persistent conversation与工具配置只在created=true时初始化，不能每次启动覆盖。
    activeSandboxValue需GET解析后显式sandbox_use；停止释放句柄不删除持久Block、history或磁盘资源。

读取与写入：
    GET一次read transaction读取全部selectors的同一snapshot并按顺序组装typed stored数组，不能逐selector独立事务。
    TOOL/SANDBOX存完整ResourceID，GET基于本次当前RAM已导入绑定快照resolve最新定义/策略；未导入显式失败、不隐式加载、不fallback旧定义。
    MESSAGE存结构化消息，保留System、usage、工具关联及递归inject等完整共享值，不只存content。
    INJECT在同一write transaction读snapshot、检查map/type/range、执行replace并commit；检查失败未修改。
    typed arrays空值仍有类型；不能把目标字段类型作为隐式转换依据。
    每次GET结果及execution提交快照不可变；资源重载只影响后续GET，不回查替换已提交工具定义/策略。

异步与取消：
    AgentBlockStorageHandle.execute拥有完整requestData及namespace，blocking worker执行AgentBlockStorage.execute。
    提交前登记scope Other；提交失败只撤销未启动登记，worker已启动必须等真实join及事务事实才mark_completed。
    MCL pending可等待/取消但不重复登记另一数据库活动；Driver停止等待Other真实收口。
    取消请求不等于rollback，更不允许丢弃Future就释放活动。commit已确认则显式已提交，未提交已确认才报告未提交，无法确认则CommitUnknown。
    success只在COMMIT确认且connection处于autocommit后返回，commit后不再检查取消；不会为消除取消竞态补写或重跑已提交命令。
    事务body失败或commit错误均显式执行ROLLBACK并检查autocommit；不以Transaction Drop忽略错误的行为证明清理。
    清理完成但commit事实未知返回CommitUnknown；rollback失败或事务未关闭返回CleanupFailed，并共享retired标记拒绝所有clone及history视图继续IO。
    CleanupFailed表示真实清理失败，不是已完成或已取消；调用方需将Other活动收口为cleanup failure，不能假mark_completed。

历史边界：
    message仅显式history_append写入；推理、tool_call、GET/INJECT不隐式append消息。
    MCL自动指令审计仍由MCL发起，其payload消息不是独立message记录。
    history_append批次自有一个事务；Block与历史不同指令之间没有跨事务原子保证，即使共文件也不能声称一起提交。
```

# events

## 逻辑

本模块不定义写入事件或转发事件，异步调用返回共享类型化结果，MCL/Agent原有请求关联、pending和生命周期结果链负责回执。不恢复旧MemoryError事件、AgentMemory写入事件、history_messages转发系统或通过观察事件证明commit/cleanup。

# types

## 共享接口

```text
AgentDatabaseHandle：共享opaque可克隆数据库能力
    history() -> AgentHistoryHandle
    block_storage() -> AgentBlockStorageHandle
    说明：两个视图共文件分表及connection manager，具体SQLite不进入共享types。

AgentBlockStorage：共享阻塞trait，Send + Sync + 'static
    execute(&self, request: BlockStorageRequest) -> Result<StoredBlockResult, BlockStorageError>
        仅blocking worker调用，完整机械契约见共享types与Block领域设计。

AgentBlockStorageHandle：共享async存储入口
    execute(&self, request: BlockStorageRequest) -> impl Future<Output = Result<StoredBlockResult, BlockStorageError>>
        调度worker且保留真实join与事务结果，不在ECS主线程IO。

StoredBlockInner：Message(Vec<AgentMessage>) / Tool(Vec<ResourceID>) / Sandbox(Vec<ResourceID>)
BlockOpened：block_id: String, created: bool, temporary: bool
    CREATE返回此值，而非无区别Unit；created仅标本namespace首次创建。

AgentHistoryStore：现有只追加共享trait，Send + Sync + 'static
    append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError>
    list(&self) -> Result<Vec<HistoryRecord>, HistoryError>
    说明：历史同步底层接口仍保留；MCL调用与读取目标在blocking worker，不再直接同步阻塞ECS。
```

## SQLite schema

历史表现有schema完整保留：

```sql
CREATE TABLE IF NOT EXISTS history_records (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE CHECK(length(key) > 0),
    source TEXT NOT NULL,
    turn TEXT,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL DEFAULT 0 CHECK(created_at_ms >= 0)
);
```

序号仅用于稳定排序，不是领域记录字段。list按sequence ASC返回完整记录快照；turn保留可选字符串，payload按JSON编码并解码为Value。created_at_ms是独立列，不放入payload冒充时间权威；message、tool schema及usage元数据由调用方按领域协议提供。新数据库不创建旧state表；打开旧数据库不删除既有表。

Block已实现schema：

```sql
CREATE TABLE IF NOT EXISTS block_definitions (
    id INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    instance TEXT NOT NULL,
    name TEXT NOT NULL CHECK(length(name) > 0),
    temporary INTEGER NOT NULL CHECK(temporary IN (0, 1)),
    schema_json TEXT NOT NULL,
    UNIQUE(agent, instance, name),
    CHECK(temporary = 1 OR instance = '')
);
CREATE TABLE IF NOT EXISTS block_fields (
    definition_id INTEGER NOT NULL REFERENCES block_definitions(id),
    name TEXT NOT NULL CHECK(length(name) > 0),
    inner_type TEXT NOT NULL,
    value_json TEXT NOT NULL,
    PRIMARY KEY(definition_id, name)
);
```

persistent的instance为空字符串；TEMP的instance为完整Driver身份tuple的JSON。schema_json保存字段名/类型的canonical BTreeMap；每field一行typed array JSON，MESSAGE保留完整消息，TOOL/SANDBOX仅保存ResourceID。open启用foreign_keys，并在Immediate事务内校验列、类型、NOT NULL、主键、外键及非partial唯一键；不兼容旧表失败且不重建。CREATE验证schema、字段数量和已存typed值；lifecycle跨instance冲突在同一写事务查询metadata后拒绝。

## 历史原子写入与幂等

先检查全部key非空，空批成功。非空批在共享连接短锁下开启Immediate事务（现实现Arc<Mutex<Connection>>），逐条按key查询，包含本事务刚写入记录，并比较除输入created_at_ms之外的HistoryRecord事实字段（当前实现先将已有记录的比较时间设为请求值，因此时间不同本身不冲突，且不更新已存时间）：

- 相同记录跳过且不生成新序号；JSON使用Value比较而非字符串比较。
- 不同记录返回KeyConflict，回滚全部新增行。
- 无已有key才INSERT并保存所有字段；全部成功才commit。
- SQL或编码失败返回WriteFailed；已确认未提交才陈述rollback，目标未知commit不能伪称未写入。

Immediate事务防止不同连接在key检查与INSERT间交错，忙等待提供等待窗口。Clone共享连接/manager，AgentHistoryHandle跨线程传递。展示层读取list后在领域/DTO转换，不从实时状态恢复拼接历史。旧AgentMemory/history_messages/set_state/state_value接口不恢复。

## 验证契约

现有测试覆盖重开持久化、JSON及turn、表边界、同批/跨批幂等、对象顺序、完整字段冲突、整批回滚、数据库失败、空key、稳定序号与线程共享；公开集成测试覆盖无Agent/Runtime外部调用及trait handle原子冲突语义。

当前 Compose/Memory scoped 验证合计 55 passed，包含一致 backup、精确 namespace 更新和全部 history 行原样保留。源码测试目标包含persistent重开完整MESSAGE（递归inject、System、工具关联、usage/cache_hit_tokens=0）及TOOL/SANDBOX canonical IDs、TEMP instance隔离及同instance幂等、跨instance lifecycle冲突保护旧TEMP、schema/range/type/SQL失败保留数据、self-source先读后替换、跨connection多selector同snapshot、async handle与history视图同manager、取消前无修改及已提交成功不翻转。坏schema测试覆盖partial UNIQUE、错误id主键、缺FK及可空列；SQLite authorizer真实拒绝ROLLBACK测试验证CleanupFailed、仍未关闭事务及两视图retired后拒绝IO。

测试没有通过注入磁盘故障证明每种CommitUnknown，也不把成功后取消测试称为精确commit中间竞态注入。Other登记/Driver停止的端到端验证属于调用方，不由memory的测试替代。

# error

## 逻辑

HistoryError保留KeyConflict/WriteFailed/InvalidRecord，新增CommitUnknown与CleanupFailed，不包含旧实时state错误。BlockStorageError区分SchemaMismatch/LifecycleMismatch/BlockMissing/FieldMissing/TypeMismatch/InvalidRange/InvalidRequest/Cancelled/CommitUnknown/CleanupFailed/Io；open返回该错误类型，不导出另一DatabaseError。资源resolve由MCL基于本次导入快照执行，不在SQLite backend内加载资源。

Cancelled仅在提交前且事务已确认结束时返回；commit I/O类错误可能返回CommitUnknown，普通SQL错误不一律升级unknown。CommitUnknown表示事务已清理、提交事实无法确认，不降级成Cancelled/已回滚；CleanupFailed表示清理无法确认且连接被retire，不允许继续IO。失败不清空持久数据、不自动销毁Agent、不自动重试或重跑外部效果。历史失败由原history_append或审计请求报告，不覆盖已完成工具真实结果。
