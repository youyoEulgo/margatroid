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

本文描述已实现的驱动加载契约，按 lib、system、handler、events、types、error 六个板块组织。
crate 保留 `agent_driver_loader_plugin` 名称；公开结构体为 `AgentDriverLoaderPlugin`，另导出 `AgentDriverPlugin` 别名。不存在另一份插件实现。

```text
lib        src/lib.rs        插件与共享驱动资源契约的导出
system     src/system.rs     一个请求/异步结果调度System
handler    src/handler.rs    两个主Handler、私有监督guard及读取辅助
events     共享types crate   agent_driver_events.rs中的请求、通知与具名异步输出
types      src/types.rs      加载配置、公开限制及私有清单文档
error      共享types crate   DriverLoadError与框架AsyncTaskError
```

# lib

## 类型

公开：
```text
AgentDriver：共享资源条目，定义于types/resource_components.rs
    base_driver: Arc<str>--base.lua的UTF-8源码
    origin: Arc<PathBuf>--base.lua来源路径
    dependencies: Arc<[ResourceID]>--清单声明的地址，不代表依赖已导入
    model: AgentDriverModelConfig--公开模型配置

AgentDriverLoaderPlugin：驱动资源插件，公开结构体
    root: PathBuf--全局驱动根目录，私有字段
    schedule: String--请求、闭包分发及挂载计划，私有字段
    limits: AgentDriverLoadLimits--读取限制，私有字段
    open(root: impl Into<PathBuf>) -> Result<Self, DriverLoadError>
        构造：要求绝对路径且无ParentDir，消除CurDir；默认PRE_UPDATE及默认限制
        不创建目录、不扫描驱动，根存在性在请求读取时检查
    with_schedule(self, schedule: impl Into<String>) -> Self
        配置计划
    with_limits(self, limits: AgentDriverLoadLimits) -> Self
        配置读取限制
    impl Plugin
        build(self, app: &mut App)
            要求Runtime、AsyncRuntime和目标计划存在，拒绝重复安装
            通过send_closure要求ClosurePlugin和该计划ClosureSystem已注册，不重复安装System
            通过受检register_resources::<ResourceSet<AgentDriver>>("driver")要求统一资源设施并注册集合
            保存AgentDriverLoadConfig，挂载一个load_agent_driver_system
            不依赖Workspace安装标记，不创建驱动宿主Entity，不注册固定异步Handler

AgentDriverPlugin：AgentDriverLoaderPlugin的公开别名
AgentDriverLoadError：DriverLoadError的公开兼容别名
```

## 逻辑

```text
AgentDriver不是Component或LibraryComponent；ResourceSet<AgentDriver>才是资源集合Component
共享条目/模型由types/resource_components.rs提供，所有驱动事件由types/agent_driver_events.rs提供
Workspace或其他存活目标Entity保存ResourceSet<AgentDriver>，ResourceID仅作为集合key
不创建AgentDriver Entity，不挂ResourceId，不登记ResourceOwners，不使用GC
插件只负责加载与指定目标挂载，不修改Workspace状态机、不启动Agent
```

# system

## 函数

crate公开：
```text
load_agent_driver_system(world: &mut World)
    克隆本帧全部LoadAgentDriver，逐个调用handle_load_agent_driver
    读取Result<AgentDriverReadOutput, AsyncTaskError>
    Ok输出克隆后逐个调用handle_mount_agent_driver
    Err记录监督错误日志；关联业务失败由CompletionGuard保障
```

## 逻辑

```text
LoadAgentDriver -> request Handler -> send_async_closure/ClosureRequest
    -> AsyncRuntime Pending -> Result<AgentDriverReadOutput, AsyncTaskError>
    -> mount Handler -> LoadAgentDriverResult
不发送裸AgentDriverReadOutput事件，不重复发送异步成功，不在异步线程访问World
LoadAgentDriverResult::Ok只在主线程受检挂载完成之后发布
```

# handler

## 函数

crate公开：
```text
handle_load_agent_driver(world: &mut World, request: LoadAgentDriver)
    检查id.trim()非空、resource_id类型为driver、完整target Entity存活
    失败直接发送保留id/target/resource_id的LoadAgentDriverResult::Err
    克隆配置，建立Arc<AtomicBool>及Arc<CompletionGuard>
    构造拥有所有参数的异步闭包，调用read_driver_files
    普通读取错误放在输出result中，外层类型为AsyncTaskError
    输出持有supervision类型擦除Arc，持续拥有guard；不提前解除监督责任
    send_async_closure提交到配置计划，每个请求独立读取，不合并同ID或不同target

handle_mount_agent_driver(world: &mut World, output: AgentDriverReadOutput)
    completed为true则跳过，避免重复挂载/回复
    普通读取失败保持目标旧集合与key不变
    成功时重新检查完整target的generation及存活性
    克隆已有ResourceSet或创建空集合，只插入/替换请求resource_id对应条目
    调用insert_resources受检入口挂载集合；失败不先删除旧集合/旧key
    发送关联LoadAgentDriverResult，发送后才completed.store(true)
```

私有：
```text
read_driver_files(root: &Path, id: &ResourceID, limits: &AgentDriverLoadLimits) -> Result<AgentDriver, DriverLoadError>
    检查root全部路径组件及scope/name/tag目录链为普通目录，拒绝symlink
    仅读取driver.toml、base.lua，不扫描、不拒绝其他目录项，包括无关symlink
    对两个自有文件执行普通文件检查、有界读取及UTF-8校验
    解析driver.toml：schema_version必须为1；inference模型配置必需
    模型名trim非空、长度有界且无控制字符
    temperature若存在需有限且位于[0,2]；top_p若存在需有限且位于[0,1]
    max_output_tokens若存在不能为0；stop数量和每项字节长度受限
    dependencies默认空，每项只允许id字段，source及其他未知依赖字段拒绝
    每个依赖id解析为ResourceID；不校验依赖存在性，不从模型配置推理依赖
    base.lua仅保存源码和origin，不编译/执行Lua，不解析IMPORT、INJECT或默认可见性
    全部成功后返回共享AgentDriver；不提交部分内容

read_text_bounded(path: &Path, maximum: u64) -> Result<String, DriverLoadError>
    symlink_metadata确认普通文件且非symlink
    open后take(maximum.saturating_add(1))，最多读取上限加一字节
    超限、I/O错误或非UTF-8均返回DriverLoadError

normalize_root(root: PathBuf) -> Result<PathBuf, DriverLoadError>
    要求绝对路径、拒ParentDir，消除CurDir，不mkdir

check_path(path: &Path, directory: bool) -> Result<(), DriverLoadError>
    使用symlink_metadata检查存在性、非symlink和期望普通类型
```

## 逻辑

```text
读取失败、目标失效、受检挂载失败都保留旧key；目标index复用不接受旧generation输出
单目标单key更新不传播其他目标；同target同key并发按挂载发生顺序覆盖，不保证最新提交胜出
只检查路径及两个自有文件；删除目录完整snapshot、mtime签名、前后版本对比与sourcehash
不承诺两文件原子快照，也不承诺对恶意并发路径替换提供无TOCTOU的安全打开
不查询推理backend/模型路由，不导入依赖，不验证Lua业务逻辑
```

# events

## 类型

公开，定义于共享types/agent_driver_events.rs：
```text
LoadAgentDriver：请求，impl Event，Clone + Debug
    id: String
    target: Entity--完整目标Entity
    resource_id: ResourceID

LoadAgentDriverResult：挂载后的通知，impl Event，Clone + Debug
    id: String
    target: Entity
    resource_id: ResourceID
    result: Result<(), DriverLoadError>--Ok表示指定target上的key已可用，不返回驱动Entity

AgentDriverReadOutput：内部异步通道具名输出，Clone
    request: LoadAgentDriver
    result: Result<AgentDriver, DriverLoadError>--使用共享条目，无额外AgentDriverData搬运层
    completed: Arc<AtomicBool>--一次性完成标记
    supervision: Arc<dyn Send + Sync>--类型擦除的私有CompletionGuard所有权
```

## 逻辑

```text
没有插件本地events.rs或重复事件；基础ClosureRequest沿用框架定义
自动异步结果为Result<AgentDriverReadOutput, AsyncTaskError>，不增加包装Event
Workspace仍通过LoadAgentDriverResult完成关联，不增加oneshot或公共LoadCompletion trait
supervision随输出及其所有Clone持续存活，避免读取完成后解除guard导致的输出丢弃窗口
```

# types

## 类型

公开：
```text
AgentDriverModelConfig：共享types/resource_components.rs中的公开结构体
    model: String
    temperature: Option<f32>
    max_output_tokens: Option<u32>
    top_p: Option<f32>
    stop: Vec<String>

AgentDriverLoadLimits：插件公开结构体，Clone + Copy + Debug
    max_manifest_bytes: u64--默认64KiB
    max_script_bytes: u64--默认4MiB
    max_model_id_bytes: usize--默认1024
    max_stop_sequences: usize--默认128
    max_stop_sequence_bytes: usize--默认4096
```

crate公开：
```text
AgentDriverLoadConfig：Clone + Resource，只保存配置，无pending表
    root: PathBuf
    schedule: String
    limits: AgentDriverLoadLimits

AgentDriverManifest：Deserialize
    schema_version: u32
    inference: AgentDriverModelConfig
    dependencies: Vec<String>--顶层字符串数组，默认空；在[inference]之前声明
    每项解析完整ResourceID，保留顺序及重复项；不接受旧依赖表或内联对象，不自动导入/启用资源
```

私有，定义于handler：
```text
CompletionGuard：关联请求监督保障
    request: LoadAgentDriver
    events: RuntimeEventSender
    completed: Arc<AtomicBool>
    impl Drop
        最后一份Arc释放时，completed.swap(true)仍为false则发送一次关联业务失败
        消息为driver task failed before mount completion
        捕获发送过程panic以避免析构二次panic；不保证Runtime退出后继续消费
    没有armed字段，没有提前解除读取阶段监督的分支
```

## 逻辑

```text
guard在闭包提交捕获阶段建立；取消或panic释放捕获内容时回复失败
正常读取后guard转入output.supervision，结果事件被丢弃时仍触发失败
正常主线程回复后设置completed，guard正常释放不再次报告
完成标记由唯一主线程挂载消费者使用；不将异步线程成功当成挂载成功
无Prepared/Payload/Mutex<Option<Output>>层，无AgentDriverData和旧模型/源码/依赖薄包装
测试覆盖实际target挂载/其他keys保留、多target、成功重载、失败保旧、generation复用、
    无关文件/目录/symlink忽略、自有symlink拒绝、两文件UTF-8及边界、清单参数/依赖字符串数组校验、缺省与空数组、顺序/重复项/tag规范化、
    open无mkdir/路径检查、guard取消/panic/输出丢弃/重复失败恰好一次回复
```

# error

## 类型

公开，定义于共享types/agent_driver_events.rs：
```text
DriverLoadError：Clone + Debug + PartialEq + Eq
    message: String--共享构造器保存文本；插件私有error helper完整保存诊断上下文
    new(message: impl Into<String>) -> Self
    impl Display + std::error::Error
        Display仅输出message
AgentDriverLoadError：插件导出的DriverLoadError兼容别名
```

## 逻辑

```text
无DriverLoadErrorKind/AgentDriverLoadErrorKind；错误以完整诊断描述关联业务请求
AsyncTaskError沿用框架，仅为外层监督错误，本身不携带原请求ID
CompletionGuard补齐关联失败，不只记日志让请求等待
不保留本地error.rs或旧AgentDriverTaskError中转层
失败不删除目标旧驱动key；读取成功仍不表示推理路由或依赖内容已经可用
```

## 可操作的加载诊断

加载失败必须保留完整 ResourceID、实际尝试的绝对文件或目录路径，以及底层 I/O、UTF-8、清单解析或校验原因；不得用笼统的“读取失败”覆盖原因，也不得在添加上下文时截断路径或原因。逐祖先检查失败同时保留最终尝试路径和失败的祖先路径。

依赖加载器在原有错误外添加资源上下文，保留上层调用方已经提供的依赖链（包括请求 id 中的关联链）；本插件不凭空推断驱动来源，也不递归加载依赖。调用方应将 `driver -> dependency` 链追加在本错误之外；Driver 清单声明的依赖仍只解析地址，不检查存在性。缺失依赖由对应 tool、prompt 或 sandbox 加载器报告其自身完整地址及实际绝对路径。

错误在异步读取、主线程挂载及业务回执之间传播时保留原分类和原因；诊断文本不再受插件的旧 512 字节/字符截断限制，公共错误类型不变。回归测试覆盖缺失驱动目录、缺失 driver.toml/base.lua，以及缺失各类依赖文件。
