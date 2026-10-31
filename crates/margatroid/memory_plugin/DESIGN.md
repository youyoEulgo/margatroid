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
# types pub  storage/core/src/types.rs                          Block、设置及历史操作
# error      storage/core/src/error.rs                          StorageError
```

# lib

## 函数

公开，定义于storage_core：
```text
open_storage(database_path: &Path, settings_path: &Path, default_settings: &str) -> Result<AgentStorage, StorageError>
    行为：打开或创建SQLite并验证schema；settings不存在时以不覆盖方式写入默认模板，存在则读取并校验；返回句柄及路径，不缓存设置正文。

execute_storage(storage: &AgentStorage, operation: StorageOperation) -> Result<StorageValue, StorageError>
    行为：串行执行对应SQLite事务或设置文件操作；写入确认完成后返回。
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
    database: StorageDatabase--私有，共享连接管理器
    settings_path: PathBuf--私有，当前Agent设置文件
    settings_lock: Arc<Mutex<()>>--私有，协调该文件读写
    impl Component for AgentStorage
    impl Clone for AgentStorage
        行为：共享连接及设置锁，不复制数据。
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
        行为：共享连接及不可用标记，不公开Connection。

StorageOperation：机械存储操作，公开枚举
    CreateBlock { name: String, fields: Vec<String>, instance: Option<String> }
    ReadMessages { selectors: Vec<MessageSelector> }
    ReplaceMessages { sources: Vec<MessageSource>, target: MessageSelector }
    ReadConfig
    SetConfig { path: String, value: toml::Value }
    DeleteConfig { path: String }
    AppendHistory { records: Vec<HistoryRecord> }
    ReadHistory

MessageSelector：消息选择器
    block: String
    field: String
    instance: Option<String>--None为持久Block，Some为本次VM临时命名空间
    selection: MessageSelection

MessageSelection：选择范围，公开枚举
    All | Index(i64) | Range(i64, i64)

MessageSource：写事务内的消息来源，公开枚举
    Stored(MessageSelector)
    Literal(Vec<serde_json::Value>)

HistoryRecord：只追加历史记录
    key: String--幂等键
    source: String--历史来源文本，不用于运行寻址
    turn: Option<String>
    kind: String
    content: String
    payload: serde_json::Value
    created_at_ms: Option<u64>--新记录缺省时取真实写入时间

StorageValue：存储成功值，公开枚举
    Unit
    Messages(Vec<serde_json::Value>)
    Config(toml::Table)
    History(Vec<HistoryRecord>)
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
    OwnerMissing | Closed | InvalidRequest | InvalidRecord | KeyConflict
    SchemaMismatch | LifecycleMismatch | BlockMissing | FieldMissing | InvalidRange
    ConfigInvalid | ConfigPathInvalid | ConfigTypeMismatch
    ReadFailed | WriteFailed | Cancelled | CommitUnknown | CleanupFailed | WorkerFailed
```

# 持有关系

```text
Agent Entity
└── AgentStorage
    ├── StorageDatabase
    │   └── SQLite连接管理器
    │       ├── 消息Block表
    │       └── 历史表
    ├── settings_path
    └── settings_lock

mecs全局异步设施
└── storage_task
    ├── owned请求及reply
    └── blocking worker
        └── SQLite事务或TOML文件操作
```
