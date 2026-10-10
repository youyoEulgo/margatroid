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
events     共享types crate    prompt_resource_events.rs中的公开事件
types      src/types.rs      加载配置、准备数据和异步关联
error      共享错误契约       提示资源加载错误
```

**本文是目标设计，不是当前实现。目录仅包含设计文档，尚未创建Cargo crate或注册Workspace成员。**

统一资源基础设施只提供ResourceID、LibraryComponent、ResourcesComponent、ResourceSet与类型注册。
本插件承接提示资源的加载与挂载，不把具体资源定义留在基础设施中。

# lib

## 组件定义位置

PromptResource资源条目及其实现方法；ResourceSet<PromptResource>使用该共享类型实例化，计划写入共享types crate的`prompt_resource_components.rs`。对应公开事件写入各自的`*_events.rs`，不在插件实现crate重复定义。

此处只标注未来源码归属，尚未创建或迁移源码文件。文档仍在lib板块描述图书馆类型和资源类型；Plugin、System、Handler及插件私有加载状态不随组件迁入共享文件。

## 类型

公开：
```text
PromptResource：提示资源内容，公开结构体--ResourceSet<PromptResource>中的一个条目，不是LibraryComponent
    alias: String--所在Entity内的本地别名
    role: String--system、user或assistant，取自prompt.toml
    content: Arc<str>--提示正文，加载时读入内存并替换

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
    具体共享源码位置在实现迁移时确定，文档中的lib归类不要求共享types反向依赖本插件

插件职责：
    不向任何消息Block自动注入提示，不执行MCL，不管理上下文或可见性。角色与内容由使用方按自身协议消费。
```

# system

## 函数

crate公开：
```text
load_prompt_resource_system(world: &mut World)
    提示资源加载System：crate公开System
    处理事件：LoadPromptResource、Result<PromptResourceReadOutput, AsyncTaskError>
    产出事件：ClosureRequest、LoadPromptResourceResult
    行为：
        克隆本帧全部LoadPromptResource，逐个调用handle_load_prompt_resource
        读取本帧全部Result<PromptResourceReadOutput, AsyncTaskError>
        成功取得异步输出时逐个调用handle_mount_prompt_resource
        外层监督错误记录日志，请求关联失败由回执保障对象发送，不让调用者永久等待
```

## 逻辑

```text
调度：
    只有一个领域System和两个主Handler，无固定异步事件Handler或额外结果收集System
    异步闭包由通用ClosureSystem分发，产出事件中的ClosureRequest指基础设施内部包装
    Pending响应完成后升变为整个Result<PromptResourceReadOutput, AsyncTaskError>，不是裸输出事件
    异步线程只准备数据，主线程挂载成功后才发LoadPromptResourceResult
```

# handler

## 函数

crate公开：
```text
handle_load_prompt_resource(world: &mut World, request: LoadPromptResource)
    处理提示加载请求：crate公开函数
    行为：
        校验请求id、alias非空、目标完整Entity存活和resource_id类型段为prompt
        请求非法时立即发送关联原请求的LoadPromptResourceResult错误结果
        复制根路径、限制与请求，建立本次关联回执保障
        在函数内构造异步闭包，读取全局根目录/prompts/scope/name/tag下的必要文件
        必要文件为prompt.toml、content.md，其他目录项忽略；不使用项目内或镜像内回退路径
        检查必要路径的目录或普通文件类型，不接受symlink，拒绝父级跳转，有界读取并要求UTF-8
        解析prompt.toml，只按明确格式读取role，要求为system、user或assistant
        有界读取content.md，要求有效UTF-8且非空
        作用域是普通资源作用域，不用system或user强行编码角色
        将role和正文存入资源条目，后续使用直接读取内存
        将准备数据或普通读取错误装入PromptResourceReadOutput，保留完整原请求与回执保障
        通过send_async_closure提交，外层错误类型为AsyncTaskError，不创建全局pending合并表

handle_mount_prompt_resource(world: &mut World, output: PromptResourceReadOutput)
    处理提示挂载结果：crate公开函数
    行为：
        已回复的请求不重复挂载或回复
        普通读取失败时发送LoadPromptResourceResult错误结果，不改变旧条目
        重新检查目标完整Entity存活，目标失效时发送TargetNotAlive，不把旧结果写入复用index的新Entity
        根据准备数据构造PromptResource，alias取原请求alias
        已有ResourceSet<PromptResource>时仅更新resource_id对应条目，不替换其他资源
        无集合时创建并插入条目，通过受检资源挂载入口挂到目标Entity
        校验或挂载失败返回MountFailed，不先删除旧集合或旧条目
        挂载完成后发送关联原请求的LoadPromptResourceResult成功通知，标记回执完成
        不修改目标的LibraryComponent，不发送其他领域的启动、执行或激活事件
```

## 逻辑

```text
更新语义：
    加载请求每次重新读取对应磁盘包，成功后替换目标Entity中的这一条资源
    普通失败不删除旧条目，但仍返回失败，不能用旧内容伪装加载成功
    同一目标同ID并发加载暂按实际挂载顺序覆盖，不保证最后提交者胜出
    不同目标的同ID资源独立更新；调用方需要内容时从指定Entity的集合读取，不取全局查询的第一个实体
    已经开始的调用如需稳定内容，由消费方在开始时取得自己的快照

生命周期：
    目标Entity销毁时集合一同释放，无ResourceOwners、release_resource_owner或GC
    只挂载内存内容或路径，不删除磁盘资源包
    加载通知表示挂载完成，调用方不重复挂载
    未收到必要结果的超时策略由调用方另行设计，不隐式销毁目标
```

# events

## 类型

公开，统一定义于共享types crate的prompt_resource_events.rs：
```text
LoadPromptResource：加载提示资源请求，公开事件
    id: String--唯一请求ID
    target: Entity--目标完整Entity，不限制为Agent或Workspace类型
    resource_id: ResourceID--prompt类型的资源地址
    alias: String--目标内本地别名，由调用方解析缺省值后提供
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
    业务请求和业务结果由共享事件层定义，本插件只使用或重导出
    成功与失败都必须保留id、target、resource_id，以便调用方推进自己的状态
    内部异步Result通道与公开加载结果不同，不重复定义ClosureRequest
    别名冲突及可见性由消费方管理，本插件只保存调用方指定的本地别名
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

PromptResourceData：异步准备数据，crate公开结构体--包含PromptResource除alias之外的已读取字段，不含World借用

PromptResourceReadOutput：异步输出，crate公开结构体
    request: LoadPromptResource--原请求
    result: Result<PromptResourceData, PromptResourceLoadError>--普通加载成功或失败
    completion: Arc<PromptResourceLoadCompletion>--从异步提交到主线程挂载的关联回执保障

PromptResourceLoadCompletion：关联回执保障，crate公开结构体
    request: LoadPromptResource--原请求上下文
    events: RuntimeEventSender--线程安全事件发送器
    completed: AtomicBool--业务结果是否已发出
    impl Drop for PromptResourceLoadCompletion
        drop(&mut self)
            兜底回复：未完成时发送携带原请求上下文的LoadPromptResourceResult错误结果，分类TaskFailed
```

## 逻辑

```text
错误关联：
    AsyncTaskError本身没有原请求id，不能只记录外层错误而丢失业务回复
    回执保障由闭包捕获并转交到输出，正常结果在主线程回复并标记完成
    panic或取消释放最后一份未完成回执时发送关联错误
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
    本次只创建目录和DESIGN.md，不迁移旧代码、不添加Cargo.toml、不修改工作区成员
```
