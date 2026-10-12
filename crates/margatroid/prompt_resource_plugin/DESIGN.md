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

正文按lib、system、handler、events、types、error六个板块组织。

```text
lib        src/lib.rs        提示资源类型与Plugin
system     src/system.rs     请求和异步结果调度
handler    src/handler.rs    提交读取与主线程挂载
events     共享types crate    prompt_resource_events.rs中的全部事件
types      src/types.rs      加载配置、准备数据和异步关联
error      共享错误契约       提示资源加载错误
```

**本文是目标设计，不是当前实现。目录仅包含设计文档，尚未创建Cargo crate或注册Workspace成员。**

统一资源基础设施只提供ResourceID、ResourceInner、LibraryComponent、ResourcesComponent、ResourceSet与类型注册。
本插件承接提示资源的加载与挂载，不把具体资源定义留在基础设施中。

# lib

## 组件定义位置

PromptResource资源条目及其实现方法；ResourceSet<PromptResource>使用该共享类型实例化，计划写入共享types crate的`prompt_resource_components.rs`。对应全部事件写入各自的`*_events.rs`，不在插件实现crate重复定义。

此处只标注未来源码归属，尚未创建或迁移源码文件。文档仍在lib板块描述图书馆类型和资源类型；Plugin、System、Handler及插件私有加载状态不随组件迁入共享文件。

## 类型

公开：
```text
PromptResource：提示资源内容，公开结构体--ResourceSet<PromptResource>中的一个条目，不是LibraryComponent
    alias: String--所在Entity内的本地别名
    role: String--system、user或assistant，取自prompt.toml
    content: Arc<str>--提示正文，加载时读入内存并替换
    impl ResourceInner for PromptResource
        get_alias(&self) -> &str
            读取本地别名：返回self.alias的字符串借用

PromptResourcePlugin：提示资源加载插件，公开结构体
    root: PathBuf--全局资源根目录，私有字段，通常为HOME/.margatroid
    schedule: String--请求、闭包分发与挂载所属计划，私有字段
    limits: PromptResourceLoadLimits--读取限制，私有字段
    open(root: impl Into<PathBuf>) -> Result<Self, PromptResourceLoadError>
        构造插件：公开关联函数，要求绝对路径且不含父级跳转，保存配置，不扫描或创建资源包
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法，保存计划名并返回自身
    impl Plugin for PromptResourcePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                要求计划、Runtime、AsyncRuntime、Closure和统一ResourcePlugin可用，拒绝重复安装
                要求目标计划已安装ClosureSystem，不重复注册
                保存PromptResourceLoadConfig，默认计划为RuntimePlugin::PRE_UPDATE
                调用register_resources::<ResourceSet<PromptResource>>("prompt")注册资源类型
                挂载load_prompt_resource_system，不注册固定异步System
                不依赖Agent、Workspace、MCL或其他资源插件的安装标记
```

## 逻辑

```text
类型与所有权：
    PromptResource在lib板块定义，辅助配置与中转数据在types板块定义
    ResourceSet<PromptResource>由统一资源设施提供Component和ResourcesComponent实现，本插件不重复实现
    ResourceID只存在于集合键中，不在条目中添加另一份ID，不挂独立ResourceId组件
    每个目标Entity有自己的资源集合，同ID可存在于多个Entity上，不创建独立资源Entity
    alias是目标Entity的本地使用名称，非全局身份；集合内一个ID只对应一个条目
    PromptResource应作为可共享的公开数据契约提供，事件层或其他消费方不需依赖本插件Handler实现
    条目与ResourceInner实现归属共享prompt_resource_components.rs；ResourceInner与集合trait归属resource_components.rs，不让共享types反向依赖本插件

插件职责：
    IMPORT的Agent依赖许可由调用方在投递加载请求前检查Agent.info.image_dependencies，按完整ResourceID精确匹配（含tag），无builtin例外
    加载插件允许任意目标Entity，只负责资源读取、校验与挂载，不重复承担Agent许可检查，也不把资源已加载视为许可
    Agent IMPORT由调用方登记Other活动后移交加载设施（不与MCL重复登记）；读取、取消和主线程挂载收口完成并真实释放资源后才mark_completed；提交失败且未启动时rollback_submission
    不向任何消息Block自动注入提示，不执行MCL，不管理上下文或可见性。角色与内容由使用方按自身协议消费。
```

# system

## 函数

crate公开：
```text
load_prompt_resource_system(world: &mut World)
    提示资源加载System：crate公开System
    处理事件：LoadPromptResource、Result<PromptResourceReadOutput, PromptResourceTaskError>
    产出事件：ClosureRequest、LoadPromptResourceResult
    行为：
        克隆本帧全部LoadPromptResource，逐个调用handle_load_prompt_resource；克隆只共享回复槽，不克隆Sender
        读取本帧全部Result<PromptResourceReadOutput, PromptResourceTaskError>
        成功取得异步输出时逐个调用handle_mount_prompt_resource
        外层监督错误调用handle_prompt_resource_task_failed，确认读取任务与附属I/O真实退出后完成失败收口并释放原Other活动；不能只记录日志或只回错
```

## 逻辑

```text
调度：
    只有一个领域System和两个主Handler，无固定异步事件Handler或额外结果收集System
    异步闭包由通用ClosureSystem分发，产出事件中的ClosureRequest指基础设施内部包装
    Pending响应完成后升变为整个Result<PromptResourceReadOutput, PromptResourceTaskError>，不是裸输出事件
    异步线程只准备数据，不能发送成功回执；通用监督设施仅在读取任务及附属I/O真实退出后投递正常输出或具名TaskError；主线程插入ResourceSet完成后才发送oneshot成功结果，并发布LoadPromptResourceResult供观察
```

# handler

## 函数

crate公开：
```text
handle_load_prompt_resource(world: &mut World, request: LoadPromptResource)
    处理提示加载请求：crate公开函数
    行为：
        校验请求id、alias非空、取消状态、目标完整Entity存活和resource_id类型段为prompt
        请求非法时完成oneshot错误回执，同时发布关联原请求的LoadPromptResourceResult错误事件
        复制根路径、限制及可复制的请求上下文，将请求中的共享reply槽交给本次关联回执保障；不复制Sender
        在函数内构造异步闭包，读取全局根目录/prompts/scope/name/tag下的必要文件
        必要文件为prompt.toml、content.md，其他目录项忽略；不使用项目内或镜像内回退路径
        检查必要路径的目录或普通文件类型，不接受symlink，拒绝父级跳转，有界读取并要求UTF-8
        解析prompt.toml，只按明确格式读取role，要求为system、user或assistant
        有界读取content.md，要求有效UTF-8且非空
        作用域是普通资源作用域，不用system或user强行编码角色
        将role和正文存入资源条目，后续使用直接读取内存
        将准备数据或普通读取错误装入PromptResourceReadOutput，保留id、target、resource_id、alias请求上下文与共享reply槽，不保留另一份Sender
        读取和解析各阶段监督request.cancel，取消时停止读取、等待本次读取资源释放并产出Cancelled关联错误，不继续准备或提交挂载
        通过send_async_closure提交，外层错误类型为PromptResourceTaskError，提交监督包装保留原请求上下文、reply及activity，不创建全局pending合并表；提交失败完成原回复槽并rollback_submission

handle_mount_prompt_resource(world: &mut World, output: PromptResourceReadOutput)
    处理提示挂载结果：crate公开函数
    行为：
        reply槽已经被take的请求不重复挂载或回复，但仍确认读取已退出并收口原activity，不能因此跳过mark_completed
        主线程再次检查output.cancel；已取消则完成Cancelled回执及观察事件且保留旧条目，禁止把取消前准备结果挂载
        取消检查与发布处于同一受检主线程收口步骤，调用方停止须等待本次收口，不以取消发送或Receiver关闭代替完成
        普通读取失败时完成oneshot错误回执并发布LoadPromptResourceResult错误事件，不改变旧条目
        重新检查目标完整Entity存活，目标失效时发送TargetNotAlive，不把旧结果写入复用index的新Entity
        根据准备数据构造PromptResource，alias取原请求alias
        已有ResourceSet<PromptResource>时仅更新resource_id对应条目，不替换其他资源
        无集合时创建并插入条目，通过受检资源挂载入口挂到目标Entity
        校验或挂载失败返回MountFailed，不先删除旧集合或旧条目
        主线程完成ResourceSet对应键的插入后，一次性发送oneshot Ok(())，并发布关联原请求的LoadPromptResourceResult成功观察事件；标记回执完成
        不修改目标的LibraryComponent，不发送其他领域的启动、执行或激活事件
        所有返回路径（成功、取消、普通失败、目标失效、reply已完成）均在确认读取资源释放并完成主线程收口后mark_completed原activity

handle_prompt_resource_task_failed(world: &mut World, error: PromptResourceTaskError)
    收口读取监督失败：crate公开函数
    行为：
        监督设施确认读取任务（含panic/abort）及附属I/O已经真实退出，才投递本关联错误；不可把发出abort或Drop当作退出证明
        主线程保留旧条目，完成尚未完成的TaskFailed回执与观察事件，随后mark_completed原activity；重复结果幂等
        若尚有未退出I/O，监督设施继续取消和等待，不提前投递可释放活动的终态；Runtime关闭前必须保持此收口设施运行
```

## 逻辑

```text
更新语义：
    重复IMPORT同一ID也必须重新发起加载，不因已存在而跳过；每次重新读取对应磁盘包，成功后仅替换目标Entity集合对应ID键的条目（包括alias）
    合理安全补充：读取、校验或挂载失败均保留原条目（含原alias），但仍返回失败，不能用旧内容伪装加载成功
    同一目标同ID并发加载暂按实际挂载顺序覆盖，不保证最后提交者胜出
    不同目标的同ID资源独立更新；调用方需要内容时从指定Entity的集合读取，不取全局查询的第一个实体
    已经开始的调用如需稳定内容，由消费方在开始时取得自己的快照

生命周期：
    目标Entity销毁时集合一同释放，无ResourceOwners、release_resource_owner或GC
    只挂载内存内容或路径，不删除磁盘资源包
    加载通知表示挂载完成，调用方不重复挂载
    调用方停止（含Agent停止）须cancel并等待读取清理和主线程挂载拒绝/完成收口；不直接丢弃IMPORT等待，也不隐式销毁目标
```

# events

## 定义位置

所有事件无论对外业务通知还是内部调度，都定义在共享types的`prompt_resource_events.rs`，插件实现不再有`events.rs`。内部仅表示用途，不意味着在共享crate中使用pub(crate)后其他crate仍能访问；供插件使用的事件需要公开可见。

自动异步响应仍是框架的`Result<T, E>`，不另造包装事件；其中具名输出、错误及请求关联载荷也在共享事件文件定义，不使共享types反向依赖插件私有类型。Plugin配置和非事件处理状态仍留在实现侧。基础设施事件如ClosureRequest沿用基础设施定义，不在业务文件复制。

请求与内部输出直接共享reply槽，完成时对槽加锁并take唯一Sender，确保一次发送；不新增公共完成trait。插件可用私有回执保障对象处理panic或取消兜底，其类型不进入共享事件契约。

## 类型

公开，统一定义于共享types crate的prompt_resource_events.rs：
```text
LoadPromptResource：加载提示资源请求，公开事件
    id: String--唯一请求ID
    target: Entity--目标完整Entity，不限制为Agent或Workspace类型
    resource_id: ResourceID--prompt类型的资源地址
    alias: String--目标内本地别名，由调用方解析缺省值后提供
    cancel: CancellationToken--调用方派生取消令牌；Agent IMPORT使用activity.cancel，供读取及主线程挂载复检
    activity: Option<ExecutionActivity>--Agent IMPORT移交的Other活动，非Agent加载可为None；加载设施接管到真实清理与主线程收口结束
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), PromptResourceLoadError>>>>>--请求携带唯一Sender的共享回复槽；克隆请求共享该槽，成功仅在主线程插入集合后发送
    impl Event for LoadPromptResource

LoadPromptResourceResult：提示资源挂载结果，公开事件
    id: String--原请求ID
    target: Entity--原目标完整Entity
    resource_id: ResourceID--原资源地址
    result: Result<(), PromptResourceLoadError>--成功表示集合条目已可用，不返回独立资源Entity
    impl Event for LoadPromptResourceResult
```

## 逻辑

```text
事件边界：
    业务请求、业务结果和内部事件均由共享事件层定义，本插件直接使用，不保留src/events.rs
    请求携带的oneshot是调用方（包括MCL）等待的完成通道；MCL不等待公开Result事件，也不把磁盘读取完成当作导入完成
    公开Result事件继续保留，仅供观察，不承担MCL等待或重新挂载职责
    成功与失败都必须保留id、target、resource_id，以便观察者关联请求
    Sender本身不可Clone；请求通过Arc共享回复槽保持可Clone，完成时加锁take一次，不能为每份事件副本创建独立Sender
    加载System仍为请求的领域处理者，观察者不得take回复槽或重复提交挂载
    内部异步Result通道与公开加载结果不同，不重复定义ClosureRequest
    本插件只保存调用方指定的本地别名；消费方准备本次可见集合时，不同完整ID的alias冲突必须失败，不按顺序覆盖或任取一项；Inference与tool_call采用相同规则
```

# types

## 类型

crate公开：
```text
PromptResourceLoadConfig：提示加载配置，crate公开Resource
    root: Arc<PathBuf>--全局根目录
    schedule: String--闭包分发计划
    limits: PromptResourceLoadLimits--读取限制
    impl Resource for PromptResourceLoadConfig

PromptResourceLoadLimits：提示读取限制，crate公开结构体
    max_definition_bytes: u64--元数据和schema等定义文件的单文件上限
    max_content_bytes: u64--content.md读取上限

PromptResourceData：异步准备数据，公开共享契约--定义于prompt_resource_events.rs--包含PromptResource除alias之外的已读取字段，不含World借用

PromptResourceReadOutput：异步输出，公开共享契约--定义于prompt_resource_events.rs，仅供内部结果通道使用
    id: String--原请求ID
    target: Entity--原目标完整Entity
    resource_id: ResourceID--原资源地址
    alias: String--原请求别名
    cancel: CancellationToken--与原请求同一派生令牌，主线程取消后拒绝挂载
    activity: Option<ExecutionActivity>--原请求同一Other活动，不新建重复登记
    result: Result<PromptResourceData, PromptResourceLoadError>--普通加载成功或失败
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), PromptResourceLoadError>>>>>--与请求共享唯一Sender槽，主线程插入集合后take并回复

PromptResourceTaskError：读取监督错误，公开共享载荷--定义于prompt_resource_events.rs
    request: LoadPromptResource--保留原id、target、resource_id、reply、cancel及activity，不依赖私有保障类型
    source: AsyncTaskError--真实退出后由监督设施生成；From<AsyncTaskError>不能凭空补齐原请求关联
    说明：异常监督包装独立持有原activity，读取Future丢弃不丢失收口责任；可取消I/O必须由同一监督设施等待，禁止脱离任务的读操作。

PromptResourceLoadCompletion：关联回执保障，私有结构体
    id: String--原请求ID
    target: Entity--原目标完整Entity
    resource_id: ResourceID--原资源地址
    alias: String--原请求别名
    cancel: CancellationToken--原请求取消状态，兜底保留关联
    activity: Option<ExecutionActivity>--兜底只完成回复，不凭Drop释放活动；真实清理及收口完成才mark_completed
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), PromptResourceLoadError>>>>>--一次性持有Sender，正常及异常路径共享同一完成门闩
    events: RuntimeEventSender--线程安全观察事件发送器
    impl Drop for PromptResourceLoadCompletion
        drop(&mut self)
            兜底回复：未完成时一次性完成oneshot Err(TaskFailed)，并发布携带原请求上下文的LoadPromptResourceResult错误观察事件；不写World，不改旧条目
```

## 逻辑

```text
错误关联：
    AsyncTaskError本身没有原请求id，不能只记录外层错误而丢失业务回复
    私有回执保障由闭包捕获；正常读取产出包含共享reply槽的内部输出，正常路径在主线程插入集合后take回复
    保障对象不得因正常读取结束先于挂载而发兜底错误；正常产出后将异常兜底责任转移给主线程结果处理，转移本身不完成回复；所有载荷仍由共享events定义
    panic或取消触发未完成回执保障时发送oneshot关联错误并发布观察事件；Drop不mark_completed，独立监督包装仍持有activity，任务真实退出后通过具名TaskError交主线程释放
    oneshot Receiver已丢弃时忽略send失败，不回滚已成功插入的条目，仍发布观察事件
    本System是该具名内部输出的唯一挂载消费者；实现必须测试各路径恰好回复一次
    Runtime整体关闭后不保证应用继续消费结果

必要的辅助实现：
    路径定位、有界读取和格式解析可用少量私有函数复用，不额外建立领域System
    不引入目录快照、修改时间比较或原子版本快照承诺
    注册表、资源集合、异步执行器复用公共基础设施，不在本插件重写
```

# error

## 类型

公开，共享错误契约：
```text
PromptResourceLoadErrorKind：提示加载错误分类，公开枚举
    InvalidRequest
    InvalidRoot
    InvalidResourceID
    NotFound
    InvalidFileType
    SymlinkNotAllowed
    LimitExceeded
    ReadFailed
    DecodeFailed
    InvalidDefinition
    TargetNotAlive
    MountFailed
    TaskFailed
    Cancelled

PromptResourceLoadError：提示资源加载错误，公开结构体
    kind: PromptResourceLoadErrorKind--错误分类
    message: String--有界错误描述
    impl Clone + Debug + PartialEq + Eq for PromptResourceLoadError
    impl fmt::Display for PromptResourceLoadError
        格式化错误：输出分类和描述
    impl std::error::Error for PromptResourceLoadError
```

## 逻辑

```text
共享契约：
    公开事件涉及的ResourceID、资源数据和错误必须能从共享层单向引用，不能让共享types反向依赖业务插件实现
    本文件只设计加载和挂载，不定义资源执行协议或修改其他插件
    本轮仅同步设计文档，不迁移旧代码、不添加Cargo.toml、不修改工作区成员
```
