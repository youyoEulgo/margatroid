# 伪代码格式
```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System区分事件、闭包结果、提交闭包与oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
方法使用下列层级；--仅承载简短说明，执行逻辑写入行为。
method(args) -> xxx
    行为：
        xxx
```

# 板块定义
```text
# lib        tool_resource/core/src/lib.rs；tool_resource/plugin/src/lib.rs  路径扩展及文件读取；Plugin安装
# components tool_resource/core/src/components.rs                         Tools统一入口集合
# events     tool_resource/core/src/events.rs                             ImportTool
# system     tool_resource/plugin/src/system.rs                           导入请求与闭包结果路由
# handler    tool_resource/plugin/src/handler.rs                          导入校验及记录登记
# types      tool_resource/plugin/src/types.rs                            导入上下文与异步载荷
# types pub  tool_resource/core/src/types.rs                              临时读取材料
# error      tool_resource/core/src/error.rs                              ToolResourceError
```

# lib

## 类型

公开，定义于tool_resource_core：
```text
WorldToolResourceExt：工具入口扩展，公开trait
    tool_entry(&self, owner: Entity, alias: &str) -> Result<&ToolEntry, ToolResourceError>
        行为：
            从该Agent的Tools按alias借用记录，不跨Agent查询。
    tool_path(&self, owner: Entity, alias: &str) -> Result<&Path, ToolResourceError>
        行为：
            借用同一ToolEntry.path。
    impl WorldToolResourceExt for World
```

公开，定义于tool_resource_plugin：
```text
ToolResourcePlugin：工具路径登记插件
    schedule: String
    impl Plugin for ToolResourcePlugin
        build(self, app: &mut App)
            行为：
                检查异步设施，拒绝重复安装；
                安装tool_resource_system。
```

## 函数

公开，定义于tool_resource_core：
```text
read_tool(path: &Path) -> Result<ToolInput, ToolResourceError>
    行为：
        完整读取tool.toml、input.schema.json、main.lua，不额外限制文件字节数
        validate 清单版本、非空描述、JSON对象schema、无外部引用、Lua UTF-8
        return ToolInput
        仅读取，不执行程序
```

# components

## 类型

公开：
```text
Tools：Agent唯一工具集合，公开组件
    entries: BTreeMap<String, ToolEntry>
    registrations: BTreeMap<String, BoundRegistration>--私有alias记录
    updates: UpdateProbe<Vec<(String, ToolEntry)>>--构造必需，绑定当前Agent来源及公共信息发布
    new(updates: UpdateProbe<Vec<(String, ToolEntry)>>) -> Self
    get(&self, alias: &str) -> Option<&ToolEntry>
        行为：
            return entries.get(alias)
    insert(&mut self, alias: String, entry: ToolEntry) -> Result<(), ToolResourceError>
        行为：
            调用同一insert_batch入口，成功后publish_current。
    insert_batch(&mut self, entries: Vec<(String, ToolEntry)>) -> Result<(), ToolResourceError>
        行为：
            同一入口校验整批schema/path/alias后原子提交；非空成功批次调用publish_current，失败不修改集合。
    insert_registered_batch(&mut self, entries: Vec<(String, ToolEntry)>, registrations: Vec<RegistrationBinding>) -> Result<(), ToolResourceError>
        行为：insert_batch复用此入口；整批校验alias/path/description/schema及绑定可用性，再同时提交entries与凭证并发布完整公共列表一次。
    remove(&mut self, alias: &str) -> Option<ToolEntry>
        行为：删除指定项并释放凭证，实际变化才发布，不扫描任务owner。
    revoke(&mut self, alias: &str, id: RegistrationId)
        行为：仅匹配当前实际注册id才删除，迟到撤销不能删除同alias新注册。
    iter(&self) -> impl Iterator<Item = (&String, &ToolEntry)>
        行为：
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
    行为：
        ImportTool -> handle_import
        Result<ImportOutput, ImportError> -> handle_import_result
```

# handler

## 函数

crate公开：
```text
handle_import(world: &mut World, request: ImportTool)
    行为：
        认领reply
        validate owner、alias、cancel
        send_async_closure(validate_import)
        failure: reply Err(error)

handle_import_result(world: &mut World, result: Result<ImportOutput, ImportError>)
    行为：
        require owner有效且未cancel
        success: Tools.insert(alias, ToolEntry{description, input_schema, path, owner_task_id=None, owner_agent=None, binding=None})
        reply 登记结果
```

私有：
```text
validate_import(context: ImportContext) -> Future<Output = Result<ImportOutput, ImportError>>
    行为：
        await spawn_blocking(验证规范路径和文件; read_tool)
        丢弃此次程序正文
        return ImportOutput(context, path, description, input_schema)
```

# types

## 类型

私有：
```text
ImportContext：导入上下文--owner、alias、path、cancel和原reply
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
    owner_task_id: Option<String>--生命周期任务id，来自已解析实际任务
    owner_agent: Option<String>--生命周期Agent的规范公开地址
    binding: Option<serde_json::Value>--工具自己的不透明路由数据，例如远端工具名

InjectedToolDefinition：注入项
    alias: String
    description: String
    input_schema: serde_json::Value
    lifetime: Option<TaskReference>--省略不绑定发起者，指定实际任务作用域
    binding: Option<serde_json::Value>
    行为：
        宿主填path为当前工具资源，owner_task_id/owner_agent由lifetime实际解析得到

ToolInput：本次工具材料
    description: String
    input_schema: serde_json::Value
    program: String
    origin: PathBuf
ToolResourceSyncBinding = ProbeFactory<Vec<(String, ToolEntry)>>
    行为：compose/DTO注入按owner创建的typed回调，转换为AgentPublicUpdate::Tools；回调仅捕获当前来源scope。
Tools::publish_current(&self)
    行为：从同一entries克隆完整公开记录后调用updates.publish，交付Agent公共视图，不发事件或借用World。

RegisterToolsRequest：Serialize + Deserialize，deny_unknown_fields
    agent: Option<String>
    tools: Vec<InjectedToolDefinition>
TaskReference：Serialize + Deserialize，deny_unknown_fields
    agent: Option<String>
    task_id: String
RegistrationId(u64)：Copy + PartialEq
    fresh() -> Self--进程内单调唯一
RegistrationBinding = (String, RegistrationId, Box<dyn Send + Sync>, Box<dyn Fn() -> bool + Send + Sync>)
BoundRegistration
    id: RegistrationId
    _guard: Box<dyn Send + Sync>
    available: Box<dyn Fn() -> bool + Send + Sync>
    行为：Tools销毁/移除释放guard解除任务登记；get/iter检查available拒绝已关闭作用域，物理删除由任务主动撤销。


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
    InvalidUtf8 | InvalidDefinition | ReadFailed | Cancelled | WorkerFailed

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
