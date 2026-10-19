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

# lib

## 源码归属与当前状态

本文记录当前源码中的加载、明确类型导入及主线程受检发布契约；本次只读核对源码，未运行测试。DTO Handler 重置后的整树恢复尚未验证，源码或测试存在不代表端到端已恢复或当前测试通过。源码按lib、system、handler、types组织，files为私有路径与读取辅助模块；所有业务及内部事件统一位于共享types，不存在插件events.rs。

组件实际归属共享`types/src/resource_components.rs`，模块路径为`margatroid_types::resource_components`；全部加载事件与错误实际归属`types/src/resource_events.rs`，模块路径为`margatroid_types::resource_events`。并非每类资源单独一个components或events文件。统一ResourceID、ResourceSet、LibraryComponent及注册入口归resource_id_plugin。

## 类型

公开：
```text
PromptResource：提示资源条目，公开共享结构体--不是LibraryComponent
    alias: String--目标Entity的本地别名
    role: String--system、user或assistant
    content: Arc<str>--加载时读取的提示正文
    impl ResourceInner for PromptResource
        get_alias(&self) -> &str
            读取别名：返回alias借用

PromptResourcePlugin：提示加载插件，公开结构体
    root: PathBuf--固定全局资源根目录
    schedule: String--请求与闭包分发计划
    limits: PromptResourceLoadLimits--读取限制
    open(root: impl Into<PathBuf>) -> Result<Self, ResourceLoadError>
        构造插件：要求绝对路径且无父级跳转；不扫描或创建资源包
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：默认RuntimePlugin::PRE_UPDATE
    with_limits(mut self, limits: PromptResourceLoadLimits) -> Self
        设置上限：保留调用方提供的读取限制
    impl Plugin for PromptResourcePlugin
        build(self, app: &mut App)
            安装插件：要求Runtime、AsyncRuntime、目标计划和已安装的ClosureSystem，拒绝重复配置；通过send_closure空闭包检查Closure设施，不重复注册ClosureSystem
            调用register_resources::<ResourceSet<PromptResource>>("prompt")；保存配置并挂一个领域System，不安装固定异步System
```

## 逻辑

```text
身份与所有权：
    ResourceID只在ResourceSet键中；同ID可独立存在于不同目标，不创建独立资源Entity或ResourceId组件
    alias不改变全局身份；import_kind为None时是普通加载，Agent及非Agent目标均不发布import_bindings
    import_kind为Some(InnerType::Message)时执行明确类型导入；声明与地址不匹配立即拒绝，目标必须是唯一解析到原完整Entity的Agent
    validate_resource_import在提交及挂载阶段调用agent_validate_import，检查driver_dependencies精确许可、alias及同ID同kind规则；不依赖AgentPlugin安装标记
    Agent.import_bindings是唯一alias绑定事实，同alias其他ID或kind冲突保旧；同ID改名先预检新名，成功发布删除旧名，不复制全局alias注册表
    本crate重导出共享条目、加载请求、观察结果、公共错误与本类读取限制；本类LoadError及LoadErrorKind是通用错误类型别名
    Tool共享条目只保存脚本路径；按需读取由执行消费侧负责，不宣称资源加载插件提供read_script异步方法
根目录：
    资源包只能来自root/prompts/scope/name/tag；没有项目、Driver目录或builtin回退
    root是通常HOME/.margatroid的全局根，不是已经包含prompts的子目录
    与AgentDriverLoaderPlugin不同：Driver插件open接受独立agent-drivers根，直接拼scope/name/tag；不能把Driver根的解释套给本插件
```

# system

## 函数

crate公开：
```text
load_prompt_resource_system(world: &mut World)
    加载System：克隆本帧LoadPromptResource，逐个提交读取
    读取Result<PromptResourceReadOutput, PromptResourceTaskError>；正常输出交mount，关联监督错误交task_failed
    一次请求一个关联上下文，不建立全局pending合并表，不因已有条目跳过重新读取
```

## 逻辑

```text
异步边界：
    使用现有WorldAsyncExt::send_async_closure；API返回()而非提交Result，并要求错误实现From<AsyncTaskError>
    闭包调度async future，future提交一个spawn_blocking只读准备工作并await专用oneshot
    独立tokio监督任务持有读取JoinHandle及原请求；等待整个读取工作真实退出，才转发成功/普通失败或带请求的TaskError
    外层dispatcher退出并丢弃接收端时，监督任务仍等待真实读取退出再发布关联TaskError；不把发出取消或Drop作为I/O退出证明
    整体Runtime关闭后不保证继续消费主线程结果，不能把这一边界描述为应用关闭后仍可靠回执
```

# handler

## 函数

crate公开：
```text
handle_load_prompt_resource(world: &mut World, request: LoadPromptResource)
    准备请求：校验id与alias非空、取消状态、完整Entity存活、resource_id类型段为prompt
    调用共享validate_resource_import；None不附加Agent导入校验，Some必须满足声明类型、完整Agent身份、依赖和alias约束
    非法或未开始请求收口回执并rollback_submission原activity，不重复登记Other活动
    复制配置与共享请求槽，构造SubmissionGuard并通过send_async_closure提交；捕获同步提交panic
    spawn_blocking工作开始前mark_started；按固定路径校验并准备资源条目
    读取prompt.toml中的role，要求system、user或assistant；有界读取content.md，要求有效UTF-8且非空。作用域不编码角色。保存内存正文。

handle_mount_prompt_resource(world: &mut World, output: PromptResourceReadOutput)
    主线程挂载：持reply槽锁检查唯一Sender是否尚在；reply已take不重复挂载或通知，但仍mark_completed原activity
    再次检查取消与完整Entity存活，取消时保旧返回Cancelled，目标复用返回TargetNotAlive
    普通读取失败保留旧条目及旧绑定；正常准备条目的alias覆盖为原请求alias
    import_kind为Some时按PromptResource.role构造ImportedValue::Message的Arc<[AgentMessage]>；system对应System、user对应User、assistant对应无tool_calls的Assistant，正文来自已读取content，不能将System降级为User。
    调用共享mount_resource_import完成单个受检主线程步骤：复检导入许可，克隆目标ResourceSet或建立空集合，仅替换原resource_id键
    借用既有ResourceTypeRegistry预检component_type、Resources分类及全部条目地址；不复制registry，ResourceSet按ID键存储，不添加重复ID
    核对ImportedValue身份和kind，保存原ImportBindings，再调用agent_bind_import；随后受检insert_resources发布集合
    insert_resources错误不修改集合，helper精确恢复本次主线程步骤前的ImportBindings；任何读取、声明、alias或挂载失败均保old set/binding
    None仅受检发布集合，不构造或修改binding；成功重载后新GET读取新快照，已返回的typed值与已提交任务保持原快照；持久SQLite Block保存资源ID，后续GET按当前导入绑定解析
    插入完成后take一次Sender发送Ok，并发布LoadPromptResourceResult观察事件；随后mark_completed原activity

handle_prompt_resource_task_failed(world: &mut World, task: PromptResourceTaskError)
    收口监督异常：只处理request为Some的关联错误，保旧回TaskFailed后mark_completed
    request为None是框架无法携带业务关联的外层错误，不能凭空猜请求或释放别人的activity；关联读取由独立监督或未开始SubmissionGuard负责
```

## 共享公开辅助函数

下列辅助实际定义在共享resource_events模块，由Tool、Prompt、Sandbox三类加载器复用，不是插件私有Handler，也不复制资源注册表。

```text
validate_resource_import(world: &World, target: Entity, resource_id: &ResourceID, alias: &str, kind: Option<InnerType>) -> Result<(), ResourceLoadError>
    导入预检：None直接成功；Some核对声明与地址、Agent组件、唯一Agent地址解析对应原Entity，并调用agent_validate_import检查许可和alias规则
mount_resource_import<T>(world: &mut World, target: Entity, resource_id: &ResourceID, alias: &str, kind: Option<InnerType>, resource: T, imported: Option<ImportedValue>) -> Result<(), ResourceLoadError>
    约束：T: Clone + Send + Sync + 'static
    原子领域发布：同一主线程步骤内预检集合及绑定，bind后受检insert_resources；后续失败精确恢复旧bindings，集合错误不会写入
    边界：调用加载器负责在持回复槽锁的挂载入口复检cancel及target存活；本helper不读取磁盘、不切换激活沙箱、不管理activity
```

# events

## 类型

公开，统一位于共享resource_events模块：
```text
ResourceLoadReply：共享回复槽，公开类型别名
    Arc<Mutex<Option<oneshot::Sender<Result<(), ResourceLoadError>>>>>

LoadPromptResource：加载请求，公开事件--Clone只共享槽与activity，不克隆Sender
    id: String--原请求ID
    target: Entity--包含generation的完整目标
    resource_id: ResourceID--本类完整地址
    alias: String--本地别名
    import_kind: Option<InnerType>--None普通加载不绑定；Some(Message)明确类型Agent导入
    cancel: CancellationToken--读取及挂载共同检查的派生取消令牌
    reply: ResourceLoadReply--唯一回复Sender槽
    activity: Option<Arc<ExecutionActivity>>--调用方登记并移交的Other活动；非Agent加载可None
    impl Event for LoadPromptResource

LoadPromptResourceResult：公开观察事件
    id: String--原请求ID
    target: Entity--原完整目标
    resource_id: ResourceID--原地址
    result: Result<(), ResourceLoadError>--成功代表主线程集合已可查询；Some导入时对应binding也已发布
    impl Event for LoadPromptResourceResult

PromptResourceReadOutput：内部正常结果载荷，公开共享结构体
    request: LoadPromptResource--保留完整原请求和共享槽
    result: Result<PromptResource, ResourceLoadError>--已准备共享条目或普通读取错误

PromptResourceTaskError：内部监督错误，公开共享结构体
    request: Option<LoadPromptResource>--独立监督产生关联错误为Some，框架转换不能补关联为None
    message: String--监督错误描述
    impl From<AsyncTaskError> for PromptResourceTaskError
        from(source: AsyncTaskError) -> Self
            框架转换：保存source字符串且request为None，不制造虚假请求
```

## 逻辑

```text
回执：
    oneshot是调用方等待的权威结果；观察事件不承担等待或再次挂载职责
    挂载步骤持共享槽锁，取消复检、受检发布、take及通知在主线程收口；调用方须cancel后等待，不直接丢弃IMPORT等待
    取消后拒绝挂载及binding发布，保原条目和绑定，等待真实读取收口后完成activity
    Receiver已丢弃时忽略send失败，不撤销已成功集合及binding发布
    外层async Result不是裸ReadOutput事件，也不新增业务ClosureRequest
```

# types

## 类型

crate公开：
```text
PromptResourceLoadConfig：私有加载配置，crate公开Resource
    root: PathBuf--固定全局根目录；现实现不是Arc<PathBuf>
    schedule: String--所属计划
    limits: PromptResourceLoadLimits--读取限制
    impl Resource for PromptResourceLoadConfig

PromptResourceLoadLimits：公开读取限制结构体--各插件采用同一字段形状，未使用的上限不产生额外文件要求
    max_definition_bytes: u64--元数据/schema单文件上限；默认1MiB
    max_content_bytes: u64--正文上限；默认8MiB
    max_script_bytes: u64--脚本加载检查上限；默认8MiB
    max_policy_bytes: u64--策略上限；默认8MiB
```

私有：
```text
SubmissionGuard：未开始提交回执保障
    request: Option<LoadPromptResource>--未移交读取监督前保留请求
    events: RuntimeEventSender--观察事件发送器
    impl Drop for SubmissionGuard
        drop(&mut self)
            未开始兜底：take唯一回复槽回TaskFailed并发布观察事件，rollback_submission而不是mark_completed；工作启动后移交关联责任并清空request
```

## 函数

私有：
```text
prepare(config: &PromptResourceLoadConfig, request: &LoadPromptResource) -> Result<PromptResource, ResourceLoadError>
    同步只读准备：在spawn_blocking工作中运行；不得借用World或发送成功回执
normalize_root(root: PathBuf) -> Result<PathBuf, ResourceLoadError>
    根路径校验：绝对路径且无ParentDir，不扫描磁盘
checked_path(path: &Path, directory: bool) -> Result<(), ResourceLoadError>
    路径校验：逐祖先symlink_metadata，禁止symlink和ParentDir，要求目录及必要普通文件类型
read_text(path: &Path, maximum: u64, cancelled: impl Fn() -> bool) -> Result<String, ResourceLoadError>
    有界读取：检查文件长度并逐8KiB块读取，检查累计上限、取消、UTF-8及非空内容
```

## 逻辑

```text
边界与测试：
    忽略非必要目录项，拒绝必要路径及祖先symlink；不承诺对恶意并发目录替换的原子快照或TOCTOU防护
    不编译Lua、不创建runner、不改LibraryComponent、不管理激活或资源GC；Entity销毁时集合自然释放
    既有测试源码描述加载成功、重复读取alias更新、失败保旧、取消前/挂载前取消、完整target复用、独立目标alias、其他key保留、reply幂等、监督错误、限长、UTF-8与symlink
    Tool测试源码另描述无效schema与外部ref；资源加载阶段不执行Lua；这些测试本次均未运行
    Tool、Prompt、Sandbox三插件联合导入回归源码描述typed GET、错误类型/alias冲突与依赖失败保旧、改名移除旧alias、已返回值/任务快照与持久Block后续GET解析当前绑定、取消不绑定、普通加载不绑定；Prompt验证角色忠实，Sandbox验证活动忙时纯重载不改选中快照
    CallableResource的Arc冻结元数据及文件路径，不冻结文件内容；不承诺以后按路径执行仍是导入时script版本或多文件原子快照
```

# error

## 类型

公开共享契约，位于resource_events模块：
```text
ResourceLoadErrorKind：公共错误分类，公开枚举
    InvalidRequest, InvalidRoot, InvalidResourceID, NotFound, InvalidFileType,
    SymlinkNotAllowed, LimitExceeded, ReadFailed, DecodeFailed, InvalidDefinition,
    TargetNotAlive, MountFailed, ExecutionBusy, TaskFailed, Cancelled

ResourceLoadError：公共加载错误，公开结构体
    kind: ResourceLoadErrorKind--分类
    message: String--new构造时最多512个Unicode字符，不宣称512字节
    new(kind: ResourceLoadErrorKind, message: impl Into<String>) -> Self
        构造错误：按字符有界截断
    kind(&self) -> ResourceLoadErrorKind
        读取分类：返回Copy分类
    impl Clone + Debug + PartialEq + Eq for ResourceLoadError
    impl fmt::Display for ResourceLoadError
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：输出分类和描述
    impl std::error::Error for ResourceLoadError

PromptResourceLoadError：兼容公开别名--等同ResourceLoadError
PromptResourceLoadErrorKind：兼容公开别名--等同ResourceLoadErrorKind
```

## 可操作的加载诊断（目标与当前缺口）

加载失败必须保留完整 ResourceID、实际尝试的绝对文件或目录路径，以及底层 I/O、UTF-8、清单解析或校验原因；不得用笼统的“读取失败”覆盖原因，也不得在添加上下文时截断路径或原因。逐祖先检查失败同时保留最终尝试路径和失败的祖先路径。

依赖加载器在原有错误外添加资源上下文，保留上层调用方已经提供的依赖链（包括请求 id 中的关联链）；本插件不凭空推断驱动来源，也不递归加载依赖。调用方应将 `driver -> dependency` 链追加在本错误之外；Driver 清单声明的依赖仍只解析地址，不检查存在性。缺失依赖由对应 tool、prompt 或 sandbox 加载器报告其自身完整地址及实际绝对路径。

目标是错误在异步读取、主线程挂载及业务回执之间传播时完整保留原分类和原因。本插件files辅助直接构造错误并保留读取/路径/上下文全文；共享导入与挂载校验使用的 `ResourceLoadError::new` 仍截取前 512 个 Unicode 字符，经过该构造器的诊断可能丢失路径或底层原因，尚不满足完整诊断目标；不得宣称已取消截断。缺失驱动目录、driver.toml/base.lua 与各类依赖文件的完整诊断仍需恢复后分别验证，本次未运行回归。
