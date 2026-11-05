# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System区分事件、闭包结果、提交闭包与oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 设计范围

Margatroid领域使用原生Axum装配及普通异步闭包。网络服务属于本应用基础设施；mecs仅提供通用运行与异步事件、闭包能力，不提供服务器封装或长期任务专用接口。

# 板块格式

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
    行为：有界读取tool.toml、input.schema.json和main.lua；校验清单版本、非空描述、JSON对象schema及Lua文件UTF-8；schema拒绝外部引用；不执行Lua。返回导入材料；导入时把description、input_schema和path存入Tools。推理不调用本文件读取函数。

```

# components

## 类型

公开：
```text
Tools：Agent唯一工具集合，公开组件
    entries: BTreeMap<String, ToolEntry>--alias到记录
    get(&self, alias: &str) -> Option<&ToolEntry>
        行为：工具调用按名称查找，存在则交执行器，不存在返回工具错误。
    insert(&mut self, alias: String, entry: ToolEntry) -> Result<(), ToolResourceError>
        行为：增加一条记录，alias冲突报错，不附加启用步骤。
    insert_batch(&mut self, entries: Vec<(String, ToolEntry)>) -> Result<(), ToolResourceError>
        行为：动态注入先校验整批，冲突整批失败；成功即所有记录出现在Tools。
    remove_owner(&mut self, owner_task_id: &str)
        行为：移除标记属于该任务的记录；任务关闭时由宿主执行。
    iter(&self) -> impl Iterator<Item = (&String, &ToolEntry)>
        行为：每次推理遍历全部记录构造schema，不接受另外的工具选择列表。
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
    行为：ImportTool进入handle_import，校验结果进入handle_import_result；最终只向Tools增加记录。core只定义数据和集合，不依赖工具执行领域。
```

# handler

## 函数

crate公开：
```text
handle_import(world: &mut World, request: ImportTool)
    行为：认领reply；校验完整owner、alias和取消状态，通过send_async_closure提交validate_import；该闭包返回本插件的Result<ImportOutput, ImportError>，由对应System收口，不再手工发送重复结果事件；失败直接回复。

handle_import_result(world: &mut World, result: Result<ImportOutput, ImportError>)
    行为：复检owner和cancel；成功时往Tools增加alias、description、input_schema、path记录，再回复Ok(())；冲突报错。

```

私有：
```text
validate_import(context: ImportContext) -> Future<Output = Result<ImportOutput, ImportError>>
    行为：spawn_blocking验证规范路径、必要文件及读取格式，调用read_tool后保留要登记的description和input_schema，丢弃此次读取的程序正文；等待真实join，保留原请求关联并返回。

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
ToolEntry：统一工具记录；普通导入与动态注入使用相同结构
    path: PathBuf--工具执行资源入口；MCP注入的记录都指向mcp_register
    description: String
    input_schema: serde_json::Value
    owner_task_id: Option<String>--动态入口标记注册任务A；普通入口None
    binding: Option<serde_json::Value>--工具自己的不透明路由数据，例如远端工具名
    行为：推理只遍历alias/description/input_schema；执行时按alias取记录，再使用path执行。不区分普通和动态的定义读取方式。

InjectedToolDefinition：注入项
    alias: String
    description: String
    input_schema: serde_json::Value
    binding: Option<serde_json::Value>
    行为：path和owner_task_id由宿主填当前资源和任务，不在模型schema/arguments中出现。

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
└── Tools：唯一工具集合
    └── BTreeMap<alias, ToolEntry>
        ├── description、input_schema、path
        └── owner_task_id、binding（注入入口的可选任务标记）

普通IMPORT
└── 读取资源、校验后向Tools增加记录

动态tool_inject
└── tool_plugin通过World闭包直接调用同一Tools的insert_batch
    └── 不另建注入事件、校验服务、提交状态或第二份定义

推理：每次遍历Tools构造schema
工具调用：按请求alias查Tools并按path执行
所有者A清理：工具监督移除Tools中owner_task_id=A的记录
```
