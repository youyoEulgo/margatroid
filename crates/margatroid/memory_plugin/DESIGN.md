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

# 板块格式

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        图书馆组件与 Plugin
# system     src/system.rs     System 函数
# handler    src/handler.rs    处理函数
# events     src/events.rs     事件类型
# types      src/types.rs      其余类型
# error      src/error.rs      Error 与公开错误分类
```

# lib

lib 放 Plugin、安装标记 Resource 和公开 re-export。

## 类型

公开：
```text
HistoryMessage：可展示历史条目，公开重导出--类型定义在agent_plugin，MemoryPlugin为兼容历史查询继续re-export
    sequence: i64--单Agent永久递增序号
    turn_id: String--原AgentMessage.id
    message: Message--User、Assistant或Tool
    tool_schema: Vec<ToolDefinition>--该Assistant产生时实际发送的内部ToolSpec；User和Tool为空
    usage: Option<TokenUsage>--Assistant行的输入、输出和缓存命中Token；User和Tool为空
    created_at_ms: i64--写入时Unix毫秒时间

MemoryPlugin：Agent消息持久化插件，公开结构体--通过事件同步历史消息和实时上下文
    schedule: String--持久化System所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，默认使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开构建方法
    impl Default for MemoryPlugin
        Default：公开trait实现，调用new
    impl Plugin for MemoryPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                重复安装时panic
                Schedule不存在时panic
                插入MemoryPluginInstalled
                在schedule依次挂载sync_history_messages_system、read_realtime_context_system和sync_realtime_context_system

MemoryPluginInstalled：MemoryPlugin安装标记，公开单元Resource--阻止重复安装并供WorkspacePlugin确认依赖
    impl Resource for MemoryPluginInstalled
```

# system

system 放 System 函数。System 只读取本帧事件并克隆，然后调用 handler。

## 函数

crate公开：
```text
sync_history_messages_system(world: &mut World)
    同步历史消息：crate公开System
    处理事件：AgentHistoryMessageWriteRequested
    行为：克隆本帧全部事件，逐个调用handle_history_message_write；失败时发送AgentMemoryWriteFailed

sync_realtime_context_system(world: &mut World)
    同步实时上下文：crate公开System
    处理事件：AgentRealtimeContextWriteRequested
    行为：克隆本帧全部事件，逐个调用handle_realtime_context_write；失败时发送AgentMemoryWriteFailed

read_realtime_context_system(world: &mut World)
    读取实时上下文：crate公开System
    处理事件：AgentRealtimeContextReadRequested
    行为：克隆本帧全部请求，逐个调用handle_realtime_context_read，并发布恰好一个AgentRealtimeContextReadCompleted
```

# handler

handler 放处理函数。每个 System 读到的领域事件在 handler 中展开为完整业务逻辑。

## 函数

crate公开：
```text
handle_store_error(error: AgentMemoryStoreError) -> MemoryError
    转换存储错误：crate公开函数，把AgentMemoryStoreError稳定分类转回MemoryError

handle_history_message_write(world: &World, event: &AgentHistoryMessageWriteRequested) -> Result<(), MemoryError>
    同步单条历史：crate公开函数
    行为：
        验证Agent存活且存在Agent.memory
        调用AgentMemoryHandle::append_history写入一条历史

handle_realtime_context_write(world: &World, event: &AgentRealtimeContextWriteRequested) -> Result<(), MemoryError>
    同步实时上下文：crate公开函数
    行为：
        验证Agent存活且存在Agent.memory
        调用AgentMemoryHandle::rewrite_realtime整体覆盖

handle_realtime_context_read(world: &World, request: &AgentRealtimeContextReadRequested) -> Result<Vec<MclMessage>, MemoryError>
    读取实时上下文：crate公开函数
    行为：从Agent.memory读取完整有序MclMessage快照
```

# events

events 放事件类型。

## 类型

公开：
```text
AgentMemoryWriteFailed：记忆写入失败事件，公开结构体
    agent: Entity--写入失败的Agent Entity
    error: MemoryError--稳定错误，不包含消息正文或完整SQL
    impl Event for AgentMemoryWriteFailed
```

# types

types 放其余类型、AgentMemory 存储实现和全部 SQLite schema/迁移/读写函数。

## 类型

公开：
```text
AgentMemory：Agent数据库打开器，公开结构体--创建Agent前建立一个独占SQLite存储并封装为共享AgentMemoryHandle
    path: PathBuf--WorkspacePlugin提供的规范化数据库路径，私有
    connection: Mutex<rusqlite::Connection>--只由MemoryPlugin使用，私有
    open(path: impl Into<PathBuf>) -> Result<(Self, RealtimeContext), MemoryError>
        打开记忆：公开关联函数
        行为：
            验证路径并创建父目录
            打开SQLite连接
            执行initialize_schema
            从realtime_context恢复有序消息投影，从history_messages恢复Token统计
    path(&self) -> &Path
        取得路径：公开方法
    history_messages(&self) -> Result<Vec<HistoryMessage>, MemoryError>
        读取展示历史：公开方法，按sequence升序恢复User、Assistant和Tool条目
    impl AgentMemoryStore for AgentMemory
        Agent存储接口：公开trait实现
        append_history：在事务中写入一条历史并提交
        rewrite_realtime：在事务中整体替换realtime_context并提交
        read_realtime：读取realtime_context完整有序MclMessage投影
        history_messages：转发给AgentMemory::history_messages

RealtimeContext：实时上下文快照，公开结构体--Agent启动时恢复的消息投影
    messages: Vec<Message>--旧公开接口保留的User和Assistant投影
    tool_context: Vec<Message>--旧公开接口保留的Tool投影
    ordered_messages: Vec<Message>--按realtime_context顺序的完整消息流
    token_usage: TokenUsage--从全部历史Assistant行聚合出的Token总数
    last_input_tokens: u64--最近一条Assistant的input_tokens
```

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
```

## 常量

私有：
```text
HISTORY_SCHEMA：建表SQL，包含history_messages和realtime_context两张表
```

## 函数

私有：
```text
memory_store_error(error: MemoryError) -> AgentMemoryStoreError
    转换存储错误：私有函数，把MemoryError包装为AgentMemoryStoreError

validate_path(path: &Path) -> Result<(), MemoryError>
    验证数据库路径：私有函数，要求路径非空且包含文件名

lock_connection(memory: &AgentMemory) -> Result<MutexGuard<Connection>, MemoryError>
    锁定连接：私有函数，锁中毒时返回WriteFailed

initialize_schema(connection: &mut Connection) -> Result<(), MemoryError>
    初始化数据库：私有函数
    行为：
        旧history_messages包含message列时改名为history_messages_legacy
        已分列但缺少tool_schema或任一Token列的history_messages改名为history_messages_layout_legacy
        旧realtime_messages没有context列时改名为realtime_messages_legacy
        创建两张业务表并为realtime_context补齐Token列
        迁移旧数据后删除legacy表和realtime_messages
        整个迁移在同一事务中完成，任一步失败时回滚

table_exists(connection: &Connection, table: &str) -> Result<bool, MemoryError>
    检查表是否存在：私有函数

table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, MemoryError>
    检查旧表字段：私有函数，通过PRAGMA table_info判断

migrate_history(transaction: &Transaction) -> Result<(), MemoryError>
    迁移旧历史：私有函数，解码旧Message JSON并按新分列schema写入

migrate_history_layout(transaction: &Transaction, layout: HistoryLayout) -> Result<(), MemoryError>
    重排分列历史：私有函数，保留sequence与已有字段，把reasoning移到content前并补缺失的tool_schema=[]与Token列=0

migrate_realtime(transaction: &Transaction) -> Result<(), MemoryError>
    迁移旧实时上下文：私有函数，把旧消息按原顺序写入realtime_context，Token列为NULL

schema_error(_: rusqlite::Error) -> MemoryError
    转换schema错误：私有函数，不暴露SQL详情

load_history_messages(connection: &Connection) -> Result<Vec<HistoryMessage>, MemoryError>
    读取历史：私有函数，按role与分列字段重建Message
    行为：Assistant恢复reasoning、tool_calls、tool_schema和Token用量；User和Tool要求tool_schema为[]且Token用量对外为空；Tool还要求tool_calls为[]并恢复resource_id与tool_call_id；任一行非法时整体失败

load_token_usage(connection: &Connection) -> Result<TokenUsage, MemoryError>
    恢复累计Token：私有函数，对role=assistant的三列分别求和，空表返回全0

load_last_input_tokens(connection: &Connection) -> Result<u64, MemoryError>
    恢复最近输入Token：私有函数，读取最后一条Assistant的input_tokens

decode_token_count(value: i64) -> Result<u64, MemoryError>
    解码Token计数：私有函数，负数返回DecodeFailed

load_realtime_context(connection: &Connection) -> Result<RealtimeContext, MemoryError>
    读取兼容投影：私有函数，按realtime_context的position升序读取，供Token恢复和旧公开接口使用

load_ordered_realtime_messages(connection: &Connection) -> Result<Vec<MclMessage>, MemoryError>
    读取有序实时消息：私有函数，要求position连续，并校验Token列要么全空要么全有

rewrite_realtime_context(transaction: &Transaction, ordered_messages: &[MclMessage]) -> Result<(), MemoryError>
    重写实时上下文：私有函数，使realtime_context与MCL request Block快照完全一致
    行为：在同一事务中删除旧行，验证消息协议并按position写入Message与可选TokenUsage

insert_history_message_values(transaction: &Transaction, turn_id: &str, message: &Message, tool_schema: &[ToolDefinition], usage: Option<&TokenUsage>, created_at_ms: i64) -> Result<(), MemoryError>
    写入历史分列：私有函数，供历史事件与旧schema迁移共用
    行为：
        User写role=user和content，tool_calls与tool_schema固定为[]，reasoning、resource_id与tool_call_id为空
        Assistant写role=assistant、可空reasoning、可空content、tool_calls和tool_schema，resource_id与tool_call_id为空
        Tool写role=tool、content、resource_id和tool_call_id，reasoning为空且tool_calls与tool_schema固定为[]
        System返回WriteFailed
        只有Assistant可以携带tool_schema和usage

current_unix_milliseconds() -> Result<i64, MemoryError>
    取得写入时间：私有函数，返回Unix毫秒并检查SQLite整数范围

read_error(_: rusqlite::Error) -> MemoryError
    转换读取错误：私有函数，不暴露SQL详情

write_error(_: rusqlite::Error) -> MemoryError
    转换写入错误：私有函数，不暴露SQL详情
```

# error

error 放 Error 类型和公开错误分类。

## 类型

公开：
```text
MemoryErrorKind：记忆错误分类，公开枚举
    InvalidPath
    DirectoryCreateFailed
    OpenFailed
    SchemaFailed
    ReadFailed
    DecodeFailed
    AgentNotAlive
    AgentMemoryMissing
    WriteFailed

MemoryError：记忆错误，公开结构体--保存稳定分类和有界描述
    kind: MemoryErrorKind--错误分类，私有
    message: String--不包含数据库内容的描述，私有
    new(kind: MemoryErrorKind, message: impl Into<String>) -> Self
        构造错误：crate公开关联函数，消息超过512字节时截断
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
```

## 逻辑

```text
安装：
    MemoryPlugin
        -> sync_history_messages_system
        -> read_realtime_context_system
        -> sync_realtime_context_system

Workspace启动：
    AgentMemory::open
        -> initialize_schema
        -> load_realtime_context
        -> load_token_usage
        -> load_last_input_tokens
        -> 返回RealtimeContext
    WorkspacePlugin
        -> 把AgentMemoryHandle放入AgentCreateRequest
    AgentPlugin
        -> 在启动Base Lua前把句柄写入Agent.memory
    Base Lua
        -> EMIT EFFECT realtime_load
        -> EMIT EFFECT realtime_source (req)

历史写入：
    Base Lua的EMIT EFFECT history_append
        -> AgentHistoryMessageWriteRequested
        -> sync_history_messages_system
        -> handle_history_message_write
        -> history_messages追加一行；Assistant写入usage，User和Tool写0

实时写入：
    Base Lua声明EMIT EFFECT realtime_source (req)后
        -> AgentRealtimeContextWriteRequested完整MclMessage快照
        -> sync_realtime_context_system
        -> handle_realtime_context_write
        -> realtime_context整体替换

读取实时：
    Base Lua的EMIT EFFECT realtime_load
        -> AgentRealtimeContextReadRequested
        -> read_realtime_context_system
        -> AgentRealtimeContextReadCompleted
```

## 边界

```text
MemoryPlugin负责：SQLite schema、历史事件写入、实时快照同步和恢复。
MemoryPlugin从Agent.memory取得存储句柄，但不维护第二份Agent状态；不判断Tool来源，不读取Skill正文，不决定上下文压缩时机。
AgentPlugin负责根据工具来源构造应写入的历史Message并发送事件。
WorkspacePlugin负责数据库路径、打开顺序和Agent绑定。
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
