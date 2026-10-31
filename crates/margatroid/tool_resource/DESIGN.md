# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System区分事件、闭包结果、提交闭包与oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib        tool_resource/core/src/lib.rs；tool_resource/plugin/src/lib.rs  路径扩展及文件读取；Plugin安装
# components tool_resource/core/src/components.rs                         Tools路径集合
# events     tool_resource/core/src/events.rs                             ImportTool
# system     tool_resource/plugin/src/system.rs                           导入请求与闭包结果路由
# handler    tool_resource/plugin/src/handler.rs                          路径校验与登记
# types      tool_resource/plugin/src/types.rs                            导入上下文与异步载荷
# types pub  tool_resource/core/src/types.rs                              临时读取材料及预算
# error      tool_resource/core/src/error.rs                              ToolResourceError
```

# lib

## 类型

公开，定义于tool_resource_core：
```text
WorldToolResourceExt：工具路径扩展，公开trait
    tool_path(&self, owner: Entity, alias: &str) -> Result<&Path, ToolResourceError>
        行为：从Tools借用路径，不读取文件。
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
    行为：有界读取tool.toml、input.schema.json和main.lua；校验清单版本、非空描述、JSON对象schema及Lua文件UTF-8；schema拒绝外部引用；不执行Lua。返回本次材料，不保存正文缓存。
```

# components

## 类型

公开：
```text
Tools：工具路径集合，公开组件
    entries: BTreeMap<String, PathBuf>--私有，alias到tool.toml规范路径
    get(&self, alias: &str) -> Option<&Path>
        行为：借用路径。
    insert(&mut self, alias: String, path: PathBuf) -> Option<PathBuf>
        行为：替换单个alias路径并返回原路径，不修改其他条目。
    remove(&mut self, alias: &str) -> Option<PathBuf>
        行为：移除路径登记。
    iter(&self) -> impl Iterator<Item = (&String, &PathBuf)>
        行为：按alias顺序遍历。
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
    行为：ImportTool -> handle_import；Result<ImportOutput, ImportError> -> handle_import_result。
```

# handler

## 函数

crate公开：
```text
handle_import(world: &mut World, request: ImportTool)
    行为：认领reply；校验完整owner、alias和取消状态，提交validate_import；失败直接回复。

handle_import_result(world: &mut World, result: Result<ImportOutput, ImportError>)
    行为：复检owner和cancel；成功时创建或更新Tools对应alias路径，再回复Ok(())；失败保持旧路径并回复错误。
```

私有：
```text
validate_import(context: ImportContext) -> Future<Output = Result<ImportOutput, ImportError>>
    行为：spawn_blocking验证规范路径、必要文件及读取格式，调用read_tool后丢弃临时正文；等待真实join，保留原请求关联并返回。
```

# types

## 类型

私有：
```text
ImportContext：导入上下文--owner、alias、path、cancel、读取预算和原reply
ImportOutput：校验成功载荷--ImportContext及规范入口路径，不包含正文
ImportError：校验失败载荷--ImportContext及ToolResourceError
```

# types pub

## 类型

公开：
```text
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
    OwnerMissing | InvalidAlias | AliasMissing | InvalidPath | FileMissing
    TooLarge | InvalidUtf8 | InvalidDefinition | ReadFailed | Cancelled | WorkerFailed
```

# 持有关系

```text
目标Entity
└── Tools
    └── BTreeMap<alias, tool.toml路径>

导入闭包
└── ImportContext
    └── 原请求oneshot发送端

使用方操作
└── ToolInput（本次读取的临时材料）
```
