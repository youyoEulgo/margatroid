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
# lib        sandbox_resource/core/src/lib.rs；sandbox_resource/plugin/src/lib.rs  路径扩展及文件读取；Plugin安装
# components sandbox_resource/core/src/components.rs                         Sandboxes与SelectedSandbox
# events     sandbox_resource/core/src/events.rs                             ImportSandbox
# system     sandbox_resource/plugin/src/system.rs                           导入请求与闭包结果路由
# handler    sandbox_resource/plugin/src/handler.rs                          路径校验与登记
# types      sandbox_resource/plugin/src/types.rs                            导入上下文与异步载荷
# types pub  sandbox_resource/core/src/types.rs                              临时读取材料及预算
# error      sandbox_resource/core/src/error.rs                              SandboxResourceError
```

# lib

## 类型

公开，定义于sandbox_resource_core：
```text
WorldSandboxResourceExt：沙箱路径扩展，公开trait
    sandbox_path(&self, owner: Entity, alias: &str) -> Result<&Path, SandboxResourceError>
        行为：从Sandboxes借用路径，不读取文件。
    sandbox_select(&mut self, owner: Entity, alias: Option<String>) -> Result<(), SandboxResourceError>
        行为：校验目标和alias已登记，调用已装配SelectedSandbox::set，不读取策略或产生ECS同步事件。
    impl WorldSandboxResourceExt for World
```

公开，定义于sandbox_resource_plugin：
```text
SandboxResourcePlugin
    schedule: String
    limits: SandboxReadLimits
    impl Plugin
        build(self, app: &mut App)
            行为：检查运行设施并拒绝重复安装，安装sandbox_resource_system；成功导入实际创建集合时必须取得SandboxSyncBinding。
```

## 函数

公开，定义于sandbox_resource_core：
```text
read_sandbox(path: &Path, limits: &SandboxReadLimits) -> Result<SandboxInput, SandboxResourceError>
    行为：有界读取policy.json并校验UTF-8及非空JSON对象；不缓存正文，执行后端校验具体策略。
```

# components

## 类型

公开：
```text
Sandboxes：沙箱路径集合，公开组件
    entries: BTreeMap<String, PathBuf>--私有alias到规范路径
    updates: UpdateProbe<Vec<(String, PathBuf)>>--构造必需，不允许缺少同步上下文
    new(updates: UpdateProbe<Vec<(String, PathBuf)>>) -> Self
    get(&self, alias: &str) -> Option<&Path>
        行为：借用路径。
    insert(&mut self, alias: String, path: PathBuf) -> Option<PathBuf>
        行为：同一insert入口替换路径，只有值变化时publish_current，返回原路径。
    remove(&mut self, alias: &str) -> Option<PathBuf>
        行为：同一remove入口删实际登记，存在时publish_current。
    iter(&self) -> impl Iterator<Item = (&String, &PathBuf)>
        行为：按alias顺序遍历。
    impl Component for Sandboxes

SelectedSandbox：当前选择，Component + Clone + Debug
    data: SelectedSandboxData--不实现DerefMut
    updates: UpdateProbe<Option<String>>
    new(alias: Option<String>, updates: UpdateProbe<Option<String>>) -> Self
    set(&mut self, alias: Option<String>)
        行为：实际改变alias后publish；重复值不发布。
    publish_current(&self)
SelectedSandboxData
    alias: Option<String>
    行为：仅只读Deref访问，无法直接赋值旁路同步。

```

# events

## 类型

公开：
```text
ImportSandbox：路径登记请求，公开事件
    owner: Entity
    alias: String
    path: PathBuf--调用方通过address解析的真实入口文件
    cancel: CancellationToken
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), SandboxResourceError>>>>>
    impl Event for ImportSandbox
```

# system

## 函数

crate公开：
```text
sandbox_resource_system(world: &mut World)
    处理事件：ImportSandbox
    接收闭包结果：Result<ImportOutput, ImportError>
    提交闭包：validate_import(ImportContext) -> Future<Output = Result<ImportOutput, ImportError>>
    产出事件：None
    oneshot回复：Result<(), SandboxResourceError>
    行为：ImportSandbox -> handle_import；Result<ImportOutput, ImportError> -> handle_import_result。
```

# handler

## 函数

crate公开：
```text
handle_import(world: &mut World, request: ImportSandbox)
    行为：认领reply；校验完整owner、alias和取消状态，通过send_async_closure提交validate_import；该闭包返回本插件的Result<ImportOutput, ImportError>，由对应System收口，不再手工发送重复结果事件；失败直接回复。

handle_import_result(world: &mut World, result: Result<ImportOutput, ImportError>)
    行为：复检owner和cancel；成功时创建或更新Sandboxes对应alias路径，再回复Ok(())；失败保持旧路径并回复错误。
```

私有：
```text
validate_import(context: ImportContext) -> Future<Output = Result<ImportOutput, ImportError>>
    行为：spawn_blocking验证规范路径、必要文件及读取格式，调用read_sandbox后丢弃临时正文；等待真实join，保留原请求关联并返回。
```

# types

## 类型

私有：
```text
ImportContext：导入上下文--owner、alias、path、cancel、读取预算和原reply
ImportOutput：校验成功载荷--ImportContext及规范入口路径，不包含正文
ImportError：校验失败载荷--ImportContext及SandboxResourceError
```

# types pub

## 类型

公开：
```text
SandboxInput：本次沙箱策略
    policy: serde_json::Value--JSON对象
    origin: PathBuf

SandboxReadLimits：读取预算
    policy_bytes: usize
SandboxSyncBinding = ProbeFactory<Vec<(String, PathBuf)>>
SandboxSelectionSyncBinding = ProbeFactory<Option<String>>
    行为：按owner创建的typed入口上下文，转换为Agent公共沙箱列表或当前选择，保持实际来源scope。
Sandboxes::publish_current(&self)
    行为：克隆同一entries的完整alias/path后调用updates.publish，不读取策略、不发ECS事件。

```

# error

## 类型

公开：
```text
SandboxResourceError：沙箱资源错误
    kind: SandboxResourceErrorKind
    path: Option<PathBuf>
    message: String
    impl Display + std::error::Error

SandboxResourceErrorKind：错误类别，公开枚举
    OwnerMissing | InvalidAlias | AliasMissing | InvalidPath | FileMissing
    TooLarge | InvalidUtf8 | InvalidDefinition | ReadFailed | Cancelled | WorkerFailed
```

# 持有关系

```text
目标Entity
├── Sandboxes
│   └── BTreeMap<alias, policy.json路径>
└── SelectedSandbox
    └── Option<alias>

导入闭包
└── ImportContext
    └── 原请求oneshot发送端

使用方操作
└── SandboxInput（本次读取的临时材料）
```
