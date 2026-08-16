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
            构建插件：拒绝重复安装，挂载sync_history_messages_system和sync_realtime_messages_system

AgentMemory：Agent数据库绑定，公开组件--一个运行中Agent独占一个SQLite连接
    path: PathBuf--WorkspacePlugin提供的规范化数据库路径，私有
    connection: Mutex<rusqlite::Connection>--只由MemoryPlugin使用，私有
    open(path: impl Into<PathBuf>) -> Result<(Self, RealtimeContext), MemoryError>
        打开记忆：公开关联函数，创建表并恢复实时上下文
        行为：返回未绑定Entity的AgentMemory及messages与tool_context，任一步失败时不返回部分结果
    path(&self) -> &Path
        取得路径：公开方法
    history_messages(&self) -> Result<Vec<HistoryMessage>, MemoryError>
        读取展示历史：公开方法，按sequence升序恢复User、Assistant和Tool条目
    impl Component for AgentMemory
        Component：公开trait实现

RealtimeContext：实时上下文快照，公开结构体--Agent启动时恢复的两类消息
    messages: Vec<Message>--长期User和Assistant对话
    tool_context: Vec<Message>--未完成当前轮的Tool上下文

HistoryMessage：可展示历史条目，公开结构体--对应history_messages中的一行
    sequence: i64--单Agent永久递增序号
    turn_id: String--原AgentMessage.id
    message: Message--User、Assistant或Tool
    tool_schema: Vec<ToolDefinition>--该Assistant产生时实际发送的内部ToolSpec；User和Tool为空
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
    AlreadyBound
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

WorldMemoryExt：World记忆扩展，公开trait--只保留Workspace启动期的Agent数据库绑定入口
    bind_agent_memory(&mut self, agent: Entity, memory: AgentMemory, context: &RealtimeContext) -> Result<(), MemoryError>
        绑定记忆：公开方法，验证Plugin、Agent和未绑定状态，使实时表与context一致后挂载AgentMemory
    impl WorldMemoryExt for World
        World记忆扩展：公开trait实现

MemoryPluginInstalled：MemoryPlugin安装标记，公开单元Resource--阻止重复安装并供WorkspacePlugin确认依赖
```

## 函数

私有：
```text
validate_path(path: &Path) -> Result<(), MemoryError>
    验证数据库路径：私有函数，要求路径非空且包含文件名

require_plugin(world: &World) -> Result<(), MemoryError>
    确认插件：私有函数，MemoryPluginInstalled不存在时返回PluginMissing

lock_connection(memory: &AgentMemory) -> Result<MutexGuard<Connection>, MemoryError>
    锁定连接：私有函数，锁中毒时返回WriteFailed

initialize_schema(connection: &mut Connection) -> Result<(), MemoryError>
    初始化数据库：私有函数，事务内迁移旧表并创建不存在的两张业务表
    行为：
        旧history_messages包含message列时改名为history_messages_legacy
        已分列但缺少tool_schema的history_messages改名为history_messages_layout_legacy
        旧realtime_messages没有context列时改名为realtime_messages_legacy
        history_messages:
            sequence INTEGER PRIMARY KEY AUTOINCREMENT
            turn_id TEXT NOT NULL
            role TEXT NOT NULL--user、assistant或tool
            reasoning TEXT--Assistant完整思考内容，User和Tool为空
            content TEXT--User和Tool为正文，Assistant可以为空
            tool_calls TEXT NOT NULL--User和Assistant的ToolCall数组JSON，Tool固定为[]
            tool_schema TEXT NOT NULL--Assistant该次推理实际ToolSpec数组JSON，User和Tool固定为[]
            resource_id TEXT--Tool具体资源ResourceId，User和Assistant为空
            tool_call_id TEXT--Tool对应调用ID，User和Assistant为空
            created_at_ms INTEGER NOT NULL
        realtime_messages:
            context TEXT NOT NULL--conversation或tool
            position INTEGER NOT NULL--同一context内从0连续递增
            message TEXT NOT NULL--Message JSON
            PRIMARY KEY(context, position)
        调用migrate_history、migrate_history_layout和migrate_realtime保留可解码内容；旧记录tool_schema固定迁移为[]；成功后删除legacy表并提交
        任一步失败时回滚整个迁移

table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, MemoryError>
    检查旧表字段：私有函数，通过PRAGMA table_info判断是否需要迁移

migrate_history(transaction: &Transaction) -> Result<(), MemoryError>
    迁移历史：私有函数，解码旧Message JSON并按新分列schema写入

migrate_history_layout(transaction: &Transaction) -> Result<(), MemoryError>
    重排分列历史：私有函数，保留sequence与全部已有字段，把reasoning移到content前并补tool_schema=[]

migrate_realtime(transaction: &Transaction) -> Result<(), MemoryError>
    迁移实时上下文：私有函数，把User和Assistant放入conversation，把Tool放入tool

schema_error(error: rusqlite::Error) -> MemoryError
    转换schema错误：私有函数，不暴露SQL详情

load_history_messages(connection: &Connection) -> Result<Vec<HistoryMessage>, MemoryError>
    读取历史：私有函数，按role与分列字段重建Message
    行为：Assistant恢复reasoning、tool_calls和tool_schema；User和Tool要求tool_schema为[]；Tool还要求tool_calls为[]并恢复resource_id与tool_call_id；任一行非法时整体失败

load_realtime_messages(connection: &Connection) -> Result<RealtimeContext, MemoryError>
    恢复实时上下文：私有函数，分别按conversation和tool的position升序读取
    行为：conversation只允许User和Assistant，tool只允许Tool；各自position必须从0连续

rewrite_realtime_messages(transaction: &Transaction, messages: &[Message], tool_context: &[Message]) -> Result<(), MemoryError>
    重写实时上下文：私有函数，使两个context分区与输入快照完全一致
    行为：在同一事务中删除旧行，验证变体并按分区从0写入，失败时由外层回滚

insert_history_message(transaction: &Transaction, event: &AgentHistoryMessageWriteRequested, created_at_ms: i64) -> Result<(), MemoryError>
    插入历史消息：私有函数，每个Message写入一行
    行为：
        User写role=user、content和tool_calls，tool_schema固定为[]，reasoning、resource_id与tool_call_id为空
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

sync_realtime_messages_system(world: &mut World)
    同步实时上下文：私有System，消费AgentContextMessagesUpdated
    行为：按事件顺序调用rewrite_realtime_messages(event.messages, event.tool_context)，不重新读取AgentContext，不修改history_messages

sync_realtime_message(world: &World, event: &AgentContextMessagesUpdated) -> Result<(), MemoryError>
    同步单次实时快照：私有函数，验证AgentMemory后在独立事务中重写并提交

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
        -> sync_realtime_messages_system

Workspace启动：
    AgentMemory::open
        -> initialize_schema
        -> load_realtime_messages
        -> 返回RealtimeContext { messages, tool_context }
    WorkspacePlugin
        -> 把两部分上下文放入AgentCreateRequest
        -> Agent创建后调用bind_agent_memory

历史写入：
    AgentToolCallSystem
        -> AgentHistoryMessageWriteRequested
        -> sync_history_messages_system
        -> history_messages追加一行
    User、Assistant和非Skill类型Tool保存原始分列内容
    Skill类型Tool历史事件的content已是完整resource_id字符串，不保存Skill正文

实时写入：
    AgentContext的messages或tool_context变更
        -> AgentContextMessagesUpdated完整快照
        -> sync_realtime_messages_system
        -> realtime_messages的conversation和tool分区整体替换
    上下文压缩只替换实时表，不删除或覆盖历史表

Agent重启：
    只从realtime_messages恢复messages和tool_context
    不从history_messages恢复模型上下文
    loading_skills不从历史推断

前端展示：
    history_messages是客户端可展示对话的唯一来源
    realtime_messages只用于Agent上下文恢复
```

## 边界

```text
MemoryPlugin负责：SQLite schema、历史事件写入、实时快照同步和恢复
MemoryPlugin不读取AgentStatus，不判断Tool来源，不读取Skill正文，不决定上下文压缩时机
AgentPlugin负责根据工具来源构造应写入的历史Message并发送事件
WorkspacePlugin负责数据库路径、打开顺序和Agent绑定
```

## 持有关系

```text
memory.sql
├── history_messages
│   └── User / Assistant / Tool
└── realtime_messages
    ├── conversation: User / Assistant
    └── tool: Tool

World
├── AgentHistoryMessageWriteRequested Event
│   └── sync_history_messages_system
├── AgentContextMessagesUpdated Event
│   └── sync_realtime_messages_system
└── Agent Entity
    └── AgentMemory
```
