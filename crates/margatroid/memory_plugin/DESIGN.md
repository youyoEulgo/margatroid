# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# MemoryPlugin

## 类型

公开：
```text
MemoryPlugin：Agent消息持久化插件，公开结构体--通过事件同步历史消息和实时上下文
    schedule: String--持久化System所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，默认使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法
    impl Plugin for MemoryPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：拒绝重复安装，注册AgentRealtimeContextWriteRequested、AgentRealtimeContextReadRequested和AgentRealtimeContextReadCompleted，挂载sync_history_messages_system、sync_realtime_context_system和read_realtime_context_system

AgentMemory：Agent数据库打开器，公开结构体--创建Agent前建立一个独占SQLite存储并封装为共享AgentMemoryHandle
    path: PathBuf--WorkspacePlugin提供的规范化数据库路径，私有
    connection: Mutex<rusqlite::Connection>--只由MemoryPlugin使用，私有
    open(path: impl Into<PathBuf>) -> Result<(AgentMemoryHandle, RealtimeContext), MemoryError>
        打开记忆：公开关联函数，创建表、按有序实时上下文恢复并从历史Assistant行聚合Token，把连接封装进AgentMemoryHandle
        行为：优先从realtime_context恢复ordered_messages；旧库没有有序表时读取旧conversation和tool分区并迁移为单一有序消息流
    path(&self) -> &Path
        取得路径：公开方法
    history_messages(&self) -> Result<Vec<HistoryMessage>, MemoryError>
        读取展示历史：公开方法，按sequence升序恢复User、Assistant和Tool条目
    impl AgentMemoryStore for AgentMemory
        Agent存储接口：公开trait实现，通过AgentMemoryHandle向MemoryPlugin的System提供历史追加、实时整体覆盖和实时读取；不暴露SQLite连接

RealtimeContext：实时上下文快照，公开结构体--Agent启动时恢复的消息投影
    ordered_messages: Vec<MclMessage>--MCL实时来源的完整有序消息流，保留Assistant的可选TokenUsage
    token_usage: TokenUsage--从全部历史Assistant行聚合出的输入、输出与缓存命中Token总数

AgentRealtimeContextWriteRequested：实时上下文整体覆盖请求，公开事件--由MclPlugin声明来源或检测到来源字段变更后产生
    agent: Entity--目标Agent Entity
    messages: Vec<MclMessage>--来源RefMerge重新展开后的完整有序快照
    impl Event + Clone for AgentRealtimeContextWriteRequested

AgentRealtimeContextReadRequested：实时上下文读取请求，公开事件--由realtime_load Effect产生
    id: String--"mcl-effect:"命名空间下的请求ID
    agent: Entity--目标Agent Entity
    impl Event + Clone for AgentRealtimeContextReadRequested

AgentRealtimeContextReadCompleted：实时上下文读取响应，公开事件--无论成功失败都恰好响应一次
    id: String--原读取请求ID
    agent: Entity--原目标Agent Entity
    result: Result<Vec<MclMessage>, MemoryError>--完整有序快照或稳定读取错误
    impl Event + Clone for AgentRealtimeContextReadCompleted

HistoryMessage：可展示历史条目，公开重导出--类型定义在agent_plugin，MemoryPlugin为兼容历史查询继续re-export
    sequence: i64--单Agent永久递增序号
    turn_id: String--原AgentMessage.id
    message: Message--User、Assistant或Tool
    tool_schema: Vec<ToolDefinition>--该Assistant产生时实际发送的内部ToolSpec；User和Tool为空
    usage: Option<TokenUsage>--Assistant行的输入、输出和缓存命中Token；User和Tool为空
    created_at_ms: i64--写入时Unix毫秒时间
    impl Clone + PartialEq for HistoryMessage
        值语义：公开trait实现

AgentMemoryWriteFailed：记忆写入失败事件，公开结构体
    agent: Entity--写入失败的Agent Entity
    error: MemoryError--稳定错误，不包含消息正文或完整SQL
    impl Event for AgentMemoryWriteFailed
        Event：公开trait实现

MemoryErrorKind：记忆错误分类，公开枚举
    InvalidPath
    DirectoryCreateFailed
    OpenFailed
    SchemaFailed
    ReadFailed
    DecodeFailed
    AgentNotAlive
    AgentMemoryMissing
    PluginMissing
    WriteFailed

MemoryError：记忆错误，公开结构体--保存稳定分类和有界描述
    kind: MemoryErrorKind--错误分类，私有
    message: String--不包含数据库内容的描述，私有
    kind(&self) -> MemoryErrorKind
        取得分类：公开方法
    message(&self) -> &str
        取得描述：公开方法
    impl fmt::Display for MemoryError
        Display：公开trait实现
    impl std::error::Error for MemoryError
        Error：公开trait实现
    impl Clone for MemoryError
        Clone：公开trait实现，供读取完成事件安全复制

MemoryPluginInstalled：MemoryPlugin安装标记，公开单元Resource--阻止重复安装并供WorkspacePlugin确认依赖
```

## 函数

私有：
```text
HistoryLayout：旧分列历史表字段能力，私有结构体--迁移时决定保留已有列或补默认值
    has_reasoning: bool
    has_resource_id: bool
    has_tool_call_id: bool
    has_tool_schema: bool
    has_input_tokens: bool
    has_output_tokens: bool
    has_cache_hit_tokens: bool

validate_path(path: &Path) -> Result<(), MemoryError>
    验证数据库路径：私有函数，要求路径非空且包含文件名

require_plugin(world: &World) -> Result<(), MemoryError>
    确认插件：私有函数，MemoryPluginInstalled不存在时返回PluginMissing

lock_connection(memory: &AgentMemory) -> Result<MutexGuard<Connection>, MemoryError>
    锁定连接：私有函数，只在AgentMemory实现AgentMemoryStore的内部使用；AgentMemoryHandle只通过AgentMemoryStore方法访问，不暴露连接

initialize_schema(connection: &mut Connection) -> Result<(), MemoryError>
    初始化数据库：私有函数，事务内迁移旧表并创建不存在的两张业务表
    行为：
        旧history_messages包含message列时改名为history_messages_legacy
        已分列但缺少tool_schema或任一Token列的history_messages改名为history_messages_layout_legacy
        旧realtime_messages没有context列时改名为realtime_messages_legacy
        history_messages:
            sequence INTEGER PRIMARY KEY AUTOINCREMENT
            turn_id TEXT NOT NULL
            role TEXT NOT NULL--user、assistant或tool
            reasoning TEXT--Assistant完整思考内容，User和Tool为空
            content TEXT--User和Tool为正文，Assistant可以为空
            tool_calls TEXT NOT NULL--Assistant的ToolCall数组JSON，User和Tool固定为[]
            tool_schema TEXT NOT NULL--Assistant该次推理实际ToolSpec数组JSON，User和Tool固定为[]
            resource_id TEXT--Tool具体资源ResourceId，User和Assistant为空
            tool_call_id TEXT--Tool对应调用ID，User和Assistant为空
            input_tokens INTEGER NOT NULL DEFAULT 0
            output_tokens INTEGER NOT NULL DEFAULT 0
            cache_hit_tokens INTEGER NOT NULL DEFAULT 0
            created_at_ms INTEGER NOT NULL
        realtime_context:
            position INTEGER PRIMARY KEY--全部实时消息的连续顺序
            message TEXT NOT NULL--Message JSON
            input_tokens INTEGER--可空，Assistant MclMessage的用量
            output_tokens INTEGER--可空，Assistant MclMessage的用量
            cache_hit_tokens INTEGER--可空，Assistant MclMessage的用量
        调用migrate_history、migrate_history_layout和migrate_realtime保留可解码内容；旧记录缺失的tool_schema固定迁移为[]，旧实时记录的Token列为NULL；成功后删除legacy表和已废弃的realtime_messages并提交
        任一步失败时回滚整个迁移

table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, MemoryError>
    检查旧表字段：私有函数，通过PRAGMA table_info判断是否需要迁移

migrate_history(transaction: &Transaction) -> Result<(), MemoryError>
    迁移历史：私有函数，解码旧Message JSON并按新分列schema写入

migrate_history_layout(transaction: &Transaction, layout: HistoryLayout) -> Result<(), MemoryError>
    重排分列历史：私有函数，保留sequence与全部已有字段，把reasoning移到content前并补缺失的tool_schema=[]与Token列=0

migrate_realtime(transaction: &Transaction) -> Result<(), MemoryError>
    迁移实时上下文：私有函数，把旧消息按原顺序写入realtime_context，Token列为NULL

schema_error(error: rusqlite::Error) -> MemoryError
    转换schema错误：私有函数，不暴露SQL详情

load_history_messages(connection: &Connection) -> Result<Vec<HistoryMessage>, MemoryError>
    读取历史：私有函数，按role与分列字段重建Message
    行为：Assistant恢复reasoning、tool_calls、tool_schema和Token用量；User和Tool要求tool_schema为[]且Token用量对外为空；Tool还要求tool_calls为[]并恢复resource_id与tool_call_id；任一行非法时整体失败

load_token_usage(connection: &Connection) -> Result<TokenUsage, MemoryError>
    恢复累计Token：私有函数，对role=assistant的三列分别求和，空表返回全0

load_realtime_context(connection: &Connection) -> Result<RealtimeContext, MemoryError>
    读取兼容投影：私有函数，按realtime_context的position升序读取，供Token恢复和旧公开接口使用

rewrite_realtime_context(transaction: &Transaction, ordered_messages: &[MclMessage]) -> Result<(), MemoryError>
    重写实时上下文：私有函数，使realtime_context与MCL request Block快照完全一致
    行为：在同一事务中删除旧行，验证消息协议并按position写入Message与可选TokenUsage

insert_history_message(transaction: &Transaction, event: &AgentHistoryMessageWriteRequested, created_at_ms: i64) -> Result<(), MemoryError>
    插入历史消息：私有函数，每个Message写入一行
    行为：
        User写role=user和content，tool_calls与tool_schema固定为[]，reasoning、resource_id与tool_call_id为空
        Assistant写role=assistant、可空reasoning、可空content、tool_calls和事件携带的tool_schema，resource_id与tool_call_id为空
        Tool写role=tool、content、resource_id和tool_call_id，reasoning为空且tool_calls与tool_schema固定为[]
        System返回WriteFailed
        MemoryPlugin不判断工具类型；Skill类型Tool历史content已由AgentPlugin替换为完整resource_id字符串，非Skill类型Tool保留完整响应正文

insert_history_message_values(transaction: &Transaction, turn_id: &str, message: &Message, tool_schema: &[ToolDefinition], created_at_ms: i64) -> Result<(), MemoryError>
    写入历史分列：私有函数，供历史事件与旧schema迁移共用

sync_history_messages_system(world: &mut World)
    同步历史消息：私有System，消费AgentHistoryMessageWriteRequested
    行为：按事件顺序分别开启事务、调用insert_history_message并提交；Agent或Memory不存在及写入失败时发送AgentMemoryWriteFailed

sync_history_message(world: &World, event: &AgentHistoryMessageWriteRequested) -> Result<(), MemoryError>
    同步单条历史：私有函数，验证AgentMemory后在独立事务中写入并提交

sync_realtime_context_system(world: &mut World)
    同步实时上下文：私有System，消费AgentRealtimeContextWriteRequested
    行为：按事件顺序从Agent.memory取得AgentMemoryHandle并调用rewrite_realtime_context(event.messages)，不读取其他Agent上下文，不修改history_messages

read_realtime_context_system(world: &mut World)
    读取实时上下文：私有System，消费AgentRealtimeContextReadRequested并发布AgentRealtimeContextReadCompleted
    行为：逐项保留原请求id和agent，从Agent.memory的AgentMemoryHandle读取完整MclMessage投影；无论成功失败都发布恰好一个Completed，由MclPlugin完成原MCL回执

current_unix_milliseconds() -> Result<i64, MemoryError>
    取得写入时间：私有函数，返回Unix毫秒并检查SQLite整数范围

read_error(error: rusqlite::Error) -> MemoryError
    转换读取错误：私有函数，不暴露SQL详情

write_error(error: rusqlite::Error) -> MemoryError
    转换写入错误：私有函数，不暴露SQL详情
```

## 逻辑

```text
安装：
    MemoryPlugin
        -> sync_history_messages_system
        -> sync_realtime_context_system
        -> read_realtime_context_system

Workspace启动：
    AgentMemory::open
        -> initialize_schema
        -> load_realtime_context（只恢复兼容投影和Token统计，不注入Agent/MCL）
        -> load_token_usage
        -> 返回RealtimeContext { ordered_messages, token_usage }
    WorkspacePlugin
        -> 把AgentMemoryHandle放入AgentCreateRequest，不把快照作为隐式上下文
    AgentPlugin
        -> 在启动Base Lua前把句柄写入Agent.memory
    Base Lua
        -> EMIT EFFECT realtime_load
        -> 将结果注入recent_conversation并执行自己的拆分策略
        -> EMIT EFFECT realtime_source (req)

历史写入：
    Base Lua的EMIT EFFECT history_append
        -> AgentHistoryMessageWriteRequested
        -> sync_history_messages_system
        -> history_messages追加一行；Assistant写入usage，User和Tool写0
    User、Assistant和非Skill类型Tool保存原始分列内容
    Skill类型Tool历史事件的content已是完整resource_id字符串，不保存Skill正文

实时写入：
    Base Lua声明EMIT EFFECT realtime_source (req)后
        -> MclPlugin保存req中唯一Message RefMerge的真实BlockPath依赖
        -> 任一依赖字段成功变化时重新展开该RefMerge
        -> AgentRealtimeContextWriteRequested完整MclMessage快照
        -> sync_realtime_context_system
        -> realtime_context整体替换
    上下文压缩只替换实时表，不删除或覆盖历史表

Agent重启：
    Base Lua显式从realtime_context恢复MclMessage快照；旧数据库仅在schema迁移时读取旧分区
    不从history_messages恢复模型上下文
    从history_messages的Assistant行恢复累计Token

前端展示：
    history_messages是客户端可展示对话的唯一来源
    realtime_context只用于Agent上下文恢复
```

## 边界

```text
MemoryPlugin负责：SQLite schema、历史事件写入、实时快照同步和恢复
MemoryPlugin从Agent.memory取得存储句柄，但不维护第二份Agent状态；不判断Tool来源，不读取Skill正文，不决定上下文压缩时机
AgentPlugin负责根据工具来源构造应写入的历史Message并发送事件
WorkspacePlugin负责数据库路径、打开顺序和Agent绑定
```

## 持有关系

```text
memory.sql
├── history_messages
│   └── User / Assistant / Tool
└── realtime_context
    └── ordered MclMessage { Message, Option<TokenUsage> }

World
├── AgentHistoryMessageWriteRequested Event
│   └── sync_history_messages_system
├── AgentRealtimeContextWriteRequested Event
│   └── sync_realtime_context_system
└── AgentRealtimeContextReadRequested Event
    └── read_realtime_context_system
└── Agent Entity
    └── Agent.memory: AgentMemoryHandle
```
