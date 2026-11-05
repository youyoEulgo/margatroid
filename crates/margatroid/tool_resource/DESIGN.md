# 伪代码格式
```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System区分事件、闭包结果、提交闭包与oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 板块定义
```text
# lib        tool_resource/core/src/lib.rs；tool_resource/plugin/src/lib.rs  路径扩展及文件读取；Plugin安装
# components tool_resource/core/src/components.rs                         Tools统一入口集合
# events     tool_resource/core/src/events.rs                             ImportTool
# system     tool_resource/plugin/src/system.rs                           导入请求与闭包结果路由
# handler    tool_resource/plugin/src/handler.rs                          导入校验及记录登记
# types      tool_resource/plugin/src/types.rs                            导入上下文与异步载荷
# types pub  tool_resource/core/src/types.rs                              临时读取材料及预算
# error      tool_resource/core/src/error.rs                              ToolResourceError
```

# lib

## 类型

公开，定义于tool_resource_core：
```text
WorldToolResourceExt：工具入口扩展，公开trait
    tool_entry(&self, owner: Entity, alias: &str) -> Result<&ToolEntry, ToolResourceError>
        行为：从该Agent的Tools按alias借用记录，不跨Agent查询。
    tool_path(&self, owner: Entity, alias: &str) -> Result<&Path, ToolResourceError>
        行为：借用同一ToolEntry.path。
    impl WorldToolResourceExt for World
```

公开，定义于tool_resource_plugin：
```text
ToolResourcePlugin：工具路径登记插件
    schedule: String
    limits: ToolReadLimits
    impl Plugin for ToolResourcePlugin
        build(self, app: &mut App)
            行为：检查异步设施，拒绝重复安装；安装tool_resource_system。
```

## 函数

公开，定义于tool_resource_core：
```text
read_tool(path: &Path, limits: &ToolReadLimits) -> Result<ToolInput, ToolResourceError>
    有界读取tool.toml、input.schema.json、main.lua
    validate 清单版本、非空描述、JSON对象schema、无外部引用、Lua UTF-8
    return ToolInput -- 仅读取，不执行程序
```

# components

## 类型

公开：
```text
Tools：Agent唯一工具集合，公开组件
    entries: BTreeMap<String, ToolEntry>--alias到记录
    get(&self, alias: &str) -> Option<&ToolEntry>
        return entries.get(alias)
    insert(&mut self, alias: String, entry: ToolEntry) -> Result<(), ToolResourceError>
        require alias未占用
        entries.insert(alias, entry)
    insert_batch(&mut self, entries: Vec<(String, ToolEntry)>) -> Result<(), ToolResourceError>
        validate 整批记录及alias冲突
        原子写入entries -- 失败不修改集合
    remove_owner(&mut self, owner_task_id: &str)
        entries.retain(entry.owner_task_id != owner_task_id)
    iter(&self) -> impl Iterator<Item = (&String, &ToolEntry)>
        return entries.iter()
    impl Component for Tools
```

# events

## 类型

公开：
```text
ImportTool：路径登记请求，公开事件
    owner: Entity
    alias: String
    path: PathBuf--调用方通过address解析的真实入口文件
    cancel: CancellationToken
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), ToolResourceError>>>>>
    impl Event for ImportTool
```

# system

## 函数

crate公开：
```text
tool_resource_system(world: &mut World)
    处理事件：ImportTool
    接收闭包结果：Result<ImportOutput, ImportError>
    提交闭包：validate_import(ImportContext) -> Future<Output = Result<ImportOutput, ImportError>>
    产出事件：None
    oneshot回复：Result<(), ToolResourceError>
    ImportTool -> handle_import
    Result<ImportOutput, ImportError> -> handle_import_result
```

# handler

## 函数

crate公开：
```text
handle_import(world: &mut World, request: ImportTool)
    认领reply
    validate owner、alias、cancel
    send_async_closure(validate_import)
    failure: reply Err(error)

handle_import_result(world: &mut World, result: Result<ImportOutput, ImportError>)
    require owner有效且未cancel
    success: Tools.insert(alias, ToolEntry{description, input_schema, path, owner_task_id=None, binding=None})
    reply 登记结果
```

私有：
```text
validate_import(context: ImportContext) -> Future<Output = Result<ImportOutput, ImportError>>
    await spawn_blocking(验证规范路径和文件; read_tool)
    return ImportOutput(context, path, description, input_schema) -- 丢弃此次程序正文
```

# types

## 类型

私有：
```text
ImportContext：导入上下文--owner、alias、path、cancel、读取预算和原reply
ImportOutput：校验成功载荷--ImportContext、规范入口路径、description及input_schema；不保存程序正文
ImportError：校验失败载荷--ImportContext及ToolResourceError
```

# types pub

## 类型

公开：
```text
ToolEntry：工具记录
    path: PathBuf--工具执行资源入口；MCP注入的记录都指向mcp_register
    description: String
    input_schema: serde_json::Value
    owner_task_id: Option<String>--动态入口标记注册任务A；普通入口None
    binding: Option<serde_json::Value>--工具自己的不透明路由数据，例如远端工具名

InjectedToolDefinition：注入项
    alias: String
    description: String
    input_schema: serde_json::Value
    binding: Option<serde_json::Value>
    -- 宿主填path和owner_task_id

ToolInput：本次工具材料
    description: String
    input_schema: serde_json::Value
    program: String
    origin: PathBuf

ToolReadLimits：读取预算
    manifest_bytes: usize
    schema_bytes: usize
    program_bytes: usize
```

# error

## 类型

公开：
```text
ToolResourceError：工具资源错误
    kind: ToolResourceErrorKind
    path: Option<PathBuf>
    message: String
    impl Display + std::error::Error

ToolResourceErrorKind：错误类别，公开枚举
    OwnerMissing | InvalidAlias | AliasMissing | AliasConflict | InvalidPath | FileMissing
    TooLarge | InvalidUtf8 | InvalidDefinition | ReadFailed | Cancelled | WorkerFailed
```

# 持有关系
```text
Agent Entity
└── Tools
    └── BTreeMap<alias, ToolEntry>
        ├── description、input_schema、path
        └── owner_task_id、binding

导入闭包
└── ImportContext
    └── 原请求oneshot发送端
```
