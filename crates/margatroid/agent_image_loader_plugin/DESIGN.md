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
lib        src/lib.rs        插件与镜像资源契约
system     src/system.rs     请求和异步结果调度
handler    src/handler.rs    提交读取、挂载结果及少量读取辅助函数
events     共享types crate    agent_image_events.rs中的公开事件
types      辅助及内部类型    模型配置、加载配置和异步结果
error      共享契约及内部错误  读取、挂载和异步任务错误
```

**本文是目标设计，代码尚未同步。crate计划由agent_image_loader_plugin改名为agent_image_plugin。本轮只改文档，不改Rust实现。**

# lib

## 组件定义位置

AgentImage资源条目及其公开关联类型；ResourceSet<AgentImage>使用该共享类型实例化，计划写入共享types crate的`agent_image_components.rs`。对应公开事件写入各自的`*_events.rs`，不在插件实现crate重复定义。

此处只标注未来源码归属，尚未创建或迁移源码文件。文档仍在lib板块描述图书馆类型和资源类型；Plugin、System、Handler及插件私有加载状态不随组件迁入共享文件。

图书馆类型与资源类型统一在lib板块定义；types板块只放辅助数据、配置和内部处理类型。

## 类型

公开：
```text
AgentImage：镜像资源内容，公开结构体--ResourceSet<AgentImage>中的一个条目
    base_driver: Arc<str>--base.lua的UTF-8源码
    origin: Arc<PathBuf>--base.lua来源路径
    dependencies: Arc<[ResourceID]>--清单声明的依赖地址，不代表资源已导入
    model: AgentImageModelConfig--模型名和可选推理参数

AgentImagePlugin：镜像资源插件，公开结构体
    root: PathBuf--全局agent-images根目录，私有字段
    schedule: String--请求受理、闭包分发和结果挂载所属计划，私有字段
    limits: AgentImageLoadLimits--读取限制，私有字段
    open(root: impl Into<PathBuf>) -> Result<Self, AgentImageLoadError>
        构造插件：公开关联函数，要求绝对路径且不含父级跳转，使用默认限制和RuntimePlugin::PRE_UPDATE
        行为：不扫描具体镜像，不加载资源；镜像根是否存在由加载时检查，不为每个请求创建目录
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法，保存计划名并返回自身
    impl Plugin for AgentImagePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                要求计划存在，Runtime、AsyncRuntime、Closure和统一资源基础设施已安装
                要求该计划已注册ClosureSystem，不重复为应用安装ClosureSystem
                拒绝重复安装，保存AgentImageLoadConfig
                调用register_resources::<ResourceSet<AgentImage>>("image")注册镜像资源集合类型
                挂载load_agent_image_system，不调用add_async_system注册固定异步Handler
                不依赖WorkspacePlugin安装标记，不创建镜像宿主Entity
```

## 逻辑

```text
资源分类：
    AgentImage是资源条目，不是图书馆组件，也不单独实现Component
    ResourceSet<AgentImage>由统一资源设施实现Component和ResourcesComponent
    image地址类型对应ResourceSet<AgentImage>，本插件负责注册该具体类型
    AgentImage类型由共享数据契约提供，Workspace和Agent可以使用它而不依赖本插件实现
    ID只存在于ResourceSet的键，不在AgentImage中再添加resourceid字段

实体组成：
    Workspace Entity
        Workspace--唯一LibraryComponent，内部保存自己的ResourceID
        ResourceSet<AgentImage>--以镜像ResourceID为键保存AgentImage
    不挂独立ResourceId组件，不创建AgentImage Entity，不登记ResourceOwners，不使用GC
```

# system

## 函数

crate公开：
```text
load_agent_image_system(world: &mut World)
    镜像加载System：crate公开System
    处理事件：LoadAgentImage、Result<AgentImageReadOutput, AsyncTaskError>
    产出事件：ClosureRequest、LoadAgentImageResult
    行为：
        克隆本帧全部LoadAgentImage，逐个调用handle_load_agent_image
        读取本帧全部Result<AgentImageReadOutput, AsyncTaskError>
        成功取得异步输出时逐个调用handle_mount_agent_image
        外层监督错误记录日志；对应请求失败由关联回执保障发送，不能只记日志而让请求永久等待
```

## 逻辑

```text
异步通道：
    提交闭包是Handler行为，实际入口事件是ClosurePlugin的ClosureRequest
    闭包分发后由AsyncRuntime创建Pending响应节点
    完成时Pending升变为Result<AgentImageReadOutput, AsyncTaskError>普通事件
    该结果在后续事件帧由本System读取，不是裸AgentImageReadOutput事件
    LoadAgentImageResult是挂载后的公开业务通知，不由异步运行时自动生成
    不手动发送第二份相同异步成功结果，不在异步线程访问World
```

# handler

## 函数

crate公开：
```text
handle_load_agent_image(world: &mut World, request: LoadAgentImage)
    处理加载请求：crate公开函数
    行为：
        校验请求id、目标完整Entity的存活性以及resource_id的image类型段
        校验失败直接发送关联原请求的LoadAgentImageResult::Err
        复制根目录、限制和原请求，建立本次请求的关联回执保障
        在函数内部构造拥有全部读取参数的异步闭包
        闭包调用read_image_files读取和解析所有镜像内容，将普通读取错误保存在输出中
        闭包返回AgentImageReadOutput，保留原请求和关联回执保障；外层错误类型为AsyncTaskError
        通过send_async_closure将闭包提交到配置的计划
        不建立按镜像ID合并并发请求的全局pending表

handle_mount_agent_image(world: &mut World, output: AgentImageReadOutput)
    处理挂载结果：crate公开函数
    行为：
        取得原请求和读取结果，已完成回复的请求不重复挂载或回复
        读取失败时发送关联原请求的LoadAgentImageResult::Err，原镜像条目不改变
        读取成功时重新检查目标完整Entity是否存活，目标失效时发送TargetNotAlive错误
        根据读取数据构造AgentImage
        目标已有ResourceSet<AgentImage>时，仅按请求resource_id插入或替换该条目
        目标没有集合时创建集合、插入条目，再通过受检资源集合挂载入口挂到目标Entity
        挂载失败发送MountFailed错误，不先删除旧集合或旧条目
        挂载完成后发送LoadAgentImageResult::Ok，表示指定目标上的条目已可用
        将本次关联回执标记为完成，避免正常释放时再次报告失败
        不修改Workspace组件、不启动Agent、不解释工作区状态机
```

私有：
```text
read_image_files(root: &Path, id: &ResourceID, limits: &AgentImageLoadLimits) -> Result<AgentImageData, AgentImageLoadError>
    读取镜像数据：私有异步函数
    行为：
        按root/scope/name/tag定位目录，检查路径链及两个必需文件为允许的普通目录和文件，不接受symlink
        只读取agent.toml和base.lua，不扫描或拒绝无关目录项
        通过read_text_bounded有界读取两个文件，要求有效UTF-8
        解析agent.toml并检查schema_version为1，模型ID非空且无控制字符
        读取模型名和可选推理参数，检查模型ID、停止序列数量和单序列长度上限
        将依赖id解析为ResourceID，依赖项只包含id，不接受source字段
        保存base.lua源码和来源路径，不编译、不执行Lua，也不解析IMPORT或INJECT语句
        全部成功后返回AgentImageData；任一失败不提交部分镜像内容

read_text_bounded(path: &Path, maximum: u64) -> Result<String, AgentImageLoadError>
    有界读取文本：私有异步函数
    行为：
        确认允许的普通文件类型，最多读取maximum加一字节
        超过上限返回LimitExceeded，读取失败返回ReadFailed，非UTF-8返回DecodeFailed
```

## 逻辑

```text
挂载与通知：
    LoadAgentImage -> 异步读取 -> 主线程挂载ResourceSet<AgentImage> -> LoadAgentImageResult
    Workspace收到成功通知只确认目标集合中存在对应ID，不再次挂载镜像
    加载失败不清空旧资源；读取全部成功且目标有效时才替换条目
    目标Entity的generation必须匹配，不将旧结果写入复用其index的新实体

并发与所有权：
    每个请求独立读取，不合并不同目标Entity的请求
    一个目标的更新不会自动传播到其他Entity的同ID条目
    同一目标同ID并发加载暂按挂载发生顺序覆盖，不保证较晚提交请求最终胜出
    若后续需要最新请求优先，应另加版本校验，不假称当前机制已保证
    组件与资源集合随目标Entity销毁，加载插件不延长目标生命周期

读取边界：
    保留基础路径检查、有界读取、UTF-8和清单校验
    删除完整目录快照、文件修改时间签名和读取前后对比，不承诺两个文件的原子版本快照
    不保留无消费方的源码SHA-256字段，不把源码哈希当成加载正确性保证
    不验证推理后端是否存在，不执行依赖资源的IMPORT，不验证Lua业务行为
```

# events

## 类型

公开，统一定义于共享types crate的agent_image_events.rs：
```text
LoadAgentImage：加载并挂载镜像请求，公开事件
    id: String--唯一加载请求ID
    target: Entity--目标完整Entity，Workspace创建流程传入Workspace Entity
    resource_id: ResourceID--image类型的镜像地址
    impl Event for LoadAgentImage

LoadAgentImageResult：镜像挂载结果通知，公开事件
    id: String--原请求ID
    target: Entity--原目标完整Entity
    resource_id: ResourceID--原镜像地址
    result: Result<(), AgentImageLoadError>--成功表示条目已挂载，不返回独立镜像Entity
    impl Event for LoadAgentImageResult
```

## 逻辑

```text
本插件只使用或重导出共享公开事件，不重复定义同名事件
ClosureRequest是通用闭包设施内部事件，不在本插件重新定义
Result<AgentImageReadOutput, AsyncTaskError>是自动异步响应类型，不是另一份公开镜像事件
结果id、target和resource_id在成功与失败两条路径都保留
```

# types

## 类型

公开：
```text
AgentImageModelConfig：镜像模型配置，公开结构体
    model: String--模型ID
    temperature: Option<f32>--可选采样温度
    max_output_tokens: Option<u32>--可选输出上限
    top_p: Option<f32>--可选核采样参数
    stop: Vec<String>--停止序列
```

crate公开：
```text
AgentImageLoadConfig：镜像加载配置，crate公开Resource--只保存配置，不保存请求注册表
    root: Arc<PathBuf>--全局镜像根目录
    schedule: String--闭包分发计划
    limits: AgentImageLoadLimits--读取限制
    impl Resource for AgentImageLoadConfig

AgentImageLoadLimits：镜像读取限制，crate公开结构体
    max_manifest_bytes: u64--清单读取上限
    max_script_bytes: u64--Lua源码读取上限，必须有界
    max_model_id_bytes: usize--模型ID长度上限
    max_stop_sequences: usize--停止序列数量上限
    max_stop_sequence_bytes: usize--单个停止序列长度上限

AgentImageData：异步准备的镜像数据，crate公开结构体--不含任何World借用
    base_driver: Arc<str>--已读取源码
    origin: Arc<PathBuf>--来源路径
    dependencies: Arc<[ResourceID]>--已解析依赖地址
    model: AgentImageModelConfig--已解析模型配置

AgentImageReadOutput：可关联的异步读取结果，crate公开结构体
    request: LoadAgentImage--原请求上下文
    result: Result<AgentImageData, AgentImageLoadError>--普通读取成功或失败
    completion: Arc<AgentImageLoadCompletion>--跨读取与挂载阶段的关联回执保障

AgentImageLoadCompletion：加载请求回执保障，crate公开结构体--在异步任务非正常结束时仍能关联失败请求
    request: LoadAgentImage--原请求上下文
    events: RuntimeEventSender--线程安全事件发送器
    completed: AtomicBool--本请求的业务结果是否已发出
    impl Drop for AgentImageLoadCompletion
        drop(&mut self)
            兜底回复：尚未完成时发送携带原请求上下文的LoadAgentImageResult::Err(TaskFailed)
```

## 逻辑

```text
简化的数据流：
    不保留AgentImageReadTask事件、全局pending等待表或并发合并列表
    删除AgentImageDependencies、AgentImageBaseDriver和AgentImageModelParameters的薄包装
    删除默认可见性字段、方法、类型及parse_default_visibility
    AgentImageData用于区分异步准备数据与最终挂载条目，不再叠加Prepared、Payload、Mutex<Option<Output>>多层搬运
    读取结果用Arc支持低成本复制；关联回执对象由异步闭包持有并转交到输出中

监督失败关联：
    AsyncTaskError本身不带原请求ID，不能只依赖From<AsyncTaskError>完成业务失败路由
    关联回执对象从任务捕获阶段一直活到挂载结束
    任务panic或取消导致捕获内容释放时，由最后一份未完成的回执对象发送关联失败
    正常读取成功或普通失败时，回执随输出存活，主线程发出业务结果后再标记完成
    本System是该内部结果类型唯一的挂载消费者，完成标记防止重复结果被再次处理
    实现时必须验证成功、读取错误、panic和取消都恰好回复一次，不能出现先标记完成后丢失结果的窗口
    Runtime整体关闭后不保证消费方继续接收，回执保障不意味着应用退出后仍可处理事件
```

# error

## 类型

公开，共享错误契约：
```text
AgentImageLoadErrorKind：镜像加载错误分类，公开枚举
    InvalidRequest
    InvalidRoot
    InvalidResourceID
    NotFound
    InvalidFileType
    SymlinkNotAllowed
    LimitExceeded
    ReadFailed
    DecodeFailed
    UnsupportedSchema
    InvalidModelConfig
    TargetNotAlive
    MountFailed
    TaskFailed

AgentImageLoadError：镜像加载错误，公开结构体
    kind: AgentImageLoadErrorKind--错误分类
    message: String--有界错误描述
    impl Clone + Debug + PartialEq + Eq for AgentImageLoadError
    impl fmt::Display for AgentImageLoadError
        格式化错误：输出分类与错误描述
    impl std::error::Error for AgentImageLoadError
```

## 逻辑

```text
错误返回：
    公开结果的错误表示请求未成功挂载，旧条目不会因此被删除
    AsyncTaskError仅作为自动异步响应的外层监督错误，不包含请求关联信息
    删除旧AgentImageTaskError业务中转层，由关联回执保障补齐监督失败通知
    不保留默认可见性、Prompt读取、BaseMcl或目录快照变化相关的旧错误分类
    错误及公开镜像数据必须可由共享事件层引用，不让共享types反向依赖本插件形成循环
```
