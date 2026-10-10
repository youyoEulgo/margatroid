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
lib        src/lib.rs        Agent图书馆组件、实现方法与Plugin
system     src/system.rs     启动和停止事件调度
handler    src/handler.rs    四个生命周期Handler
events     共享types crate    agent_events.rs中的公开事件
types      src/types.rs及共享契约    状态机、配置、历史接口和运行时Block容器
error      共享错误契约       生命周期与历史写入错误
```

**本文是目标设计，尚未同步到Rust代码。本次只改Agent设计文档，不实现其他插件。**

# lib

## 组件定义位置

Agent图书馆组件及其实现方法；AgentInfo、AgentState等组件公开关联类型随组件契约组织，计划写入共享types crate的`agent_components.rs`。对应公开事件写入各自的`*_events.rs`，不在插件实现crate重复定义。

此处只标注未来源码归属，尚未创建或迁移源码文件。文档仍在lib板块描述图书馆类型和资源类型；Plugin、System、Handler及插件私有加载状态不随组件迁入共享文件。

## 类型

公开：
```text
Agent：Agent图书馆组件，公开Component--自身身份和运行数据的唯一领域组件
    resourceid: ResourceID--Agent自身地址
    info: AgentInfo--从启动镜像和成员定义取得的静态配置快照
    workspace: ResourceID--所属工作区的逻辑地址
    sandbox: Option<ResourceID>--当前激活的沙箱地址，None表示未激活
    state: AgentState--Agent生命周期及当前启动或停止操作
    lua: Option<DriverLuaVMHandle>--Driver VM查询句柄，不复制VM状态
    history: AgentHistoryHandle--历史表句柄，不含实时表或Block持久化接口
    block: AgentBlock--运行时Block容器
    append_history(&self, record: HistoryRecord) -> Result<(), HistoryError>
        追加历史：公开方法
        行为：
            调用自身history句柄追加一条记录
            返回写入结果，不覆盖旧历史，不维护实时表
            写入失败不在此停止VM或销毁Agent，由调用方处理错误
    switch_sandbox(&mut self, resourceid: Option<ResourceID>)
        切换沙箱：公开方法
        行为：
            将sandbox更新为传入地址，None表示取消激活
            不加载资源、不派生进程、不追溯改变已开始调用的策略快照
        前置条件：调用方已检查非空地址的sandbox类型，以及该Agent实体的ResourceSet<SandboxResource>中存在对应条目
    impl Component for Agent
    impl LibraryComponent for Agent
        LibraryComponent：公开trait实现
        resource_id(&self) -> &ResourceID
            读取自身地址：返回resourceid字段

AgentPlugin：Agent生命周期插件，公开结构体
    schedule: String--System所属运行计划
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法，保存计划名并返回自身
    impl Default for AgentPlugin
        default() -> Self
            默认构造：使用RuntimePlugin::UPDATE
    impl Plugin for AgentPlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                要求运行计划、公共事件与统一资源基础设施可用，拒绝重复安装
                调用register_library::<Agent>("agent")注册图书馆类型
                挂载start_agent_system和stop_agent_system
                不注册资源类型，tool、skill、hook、prompt、sandbox分别由其资源插件注册
                不直接依赖Workspace、AgentImage或Driver VM插件的Handler和安装标记
```

## 逻辑

```text
实体组成：
    Agent Entity
        Agent--唯一LibraryComponent，内部保存resourceid
        ResourceSet<ToolResource>
        ResourceSet<SkillResource>
        ResourceSet<HookResource>
        ResourceSet<PromptResource>
        ResourceSet<SandboxResource>
    五个集合均为ResourcesComponent，可与Agent共同挂载
    启动时挂载空集合，不等于把镜像依赖全部预加载；后续导入通过各资源加载插件更新
    不挂独立ResourceId组件，不使用ResourceRef、ResourceOwners或资源GC

数据归属：
    info是启动配置快照，源镜像后续更新不自动修改运行中的Agent
    workspace只记录逻辑地址，异步结果按完整Agent Entity和子请求ID关联
    lua只保留定位VM的句柄，实际VM状态由VM领域提供查询，AgentState不复制它
    block包含普通Block和RefBlock，不因字段合并删掉现有引用表达能力
    历史句柄只提供追加历史能力，不沿用带实时state读写的旧AgentMemoryHandle

方法边界：
    append_history和switch_sandbox是Agent实现方法，不是独立Handler或System
    Agent不能自行查询同Entity上的兄弟组件，沙箱存在性由调用方先检查
    调用方先以World不可变借用检查集合，再取Agent可变借用调用switch_sandbox，不向组件方法传入同一World的可变借用
    资源集合是资源数据来源，Agent不复制工具描述、schema、正文或沙箱策略

MCL边界：
    不存在AgentControl入口，不转发MCL，不恢复长期VM邮箱投递
    MCL直接定位Agent并通过block容器的机械方法操作字段
    Agent插件不解释Block名称和内容用途，不代替MCL做语法解析
```

# system

## 函数

crate公开：
```text
start_agent_system(world: &mut World)
    启动System：crate公开System
    处理事件：StartAgent、StartDriverLuaVMResult
    产出事件：StartDriverLuaVM、StartAgentResult
    行为：
        克隆本帧全部StartAgent，逐个调用handle_start_agent
        克隆本帧全部StartDriverLuaVMResult，逐个调用handle_start_driver_lua_vm_result

stop_agent_system(world: &mut World)
    停止System：crate公开System
    处理事件：StopAgent、StopDriverLuaVMResult
    产出事件：StopDriverLuaVM、StopAgentResult
    行为：
        克隆本帧全部StopAgent，逐个调用handle_stop_agent
        克隆本帧全部StopDriverLuaVMResult，逐个调用handle_stop_driver_lua_vm_result
```

## 逻辑

```text
System只读取事件并调度Handler，不自行操作VM或展开清理流程
无事件时不轮询Agent是否停止，不查询工具、推理或VM内部状态拼接停止判据
资源集合更新由各资源插件处理，不添加Agent资源收集System
不保留agent_control_system、agent_message_system或旧agent_lua_vm_state_system
```

# handler

## 函数

crate公开：
```text
handle_start_agent(world: &mut World, request: StartAgent)
    处理启动请求：crate公开函数
    行为：
        校验请求ID、Agent地址类型、成员定义和目标工作区完整Entity
        按Agent地址查询已有实例，存在时返回DuplicateAgent，不覆盖旧实例
        从指定工作区Entity上的ResourceSet<AgentImage>按镜像ResourceID取得启动材料，不选择全局查询的首个匹配者
        取得模型配置、项目路径、Driver源码及声明的依赖地址，构造独立启动快照
        准备历史表句柄，失败时返回HistorySetupFailed，不发VM请求
        创建Agent Entity，通过受检图书馆入口挂载Agent，并挂载五种空资源集合
        info保存配置，workspace保存工作区ResourceID，sandbox和lua初始为空，block为空运行时容器
        生成VM启动子请求ID，先将原StartAgent请求ID和子请求ID写入Creating
        整理注入环境，包含完整Agent Entity、Agent地址、启动配置及所需环境能力，提交StartDriverLuaVM
        提交VM前的任何失败均释放本次已创建Entity和句柄，发送关联原请求的StartAgentResult::Err
        不在发出VM请求后提前返回启动成功

handle_start_driver_lua_vm_result(world: &mut World, event: StartDriverLuaVMResult)
    处理Driver启动结果：crate公开函数
    行为：
        核对目标完整Agent Entity、Creating状态和VM启动子请求ID，旧结果或重复结果不推进其他实例
        成功时保存VM查询句柄，将state改为Running，发送原请求ID对应的StartAgentResult::Ok(agent)
        失败时确认VM结果契约保证本次执行体已清理，保存原请求上下文，销毁Agent并发送StartAgentResult::Err
        若失败无法保证没有遗留执行体，不直接销毁宿主；此类结果必须由Driver VM插件完成清理后再返回
        每个合法受理的启动请求无论成功失败都必须有结果；无关或重复事件不重复回复

handle_stop_agent(world: &mut World, request: StopAgent)
    处理停止请求：crate公开函数
    行为：
        检查目标Entity和Agent，不存在时记录warning并发送AgentNotAlive错误
        Creating时暂定返回AgentBusy，不抢先销毁正在等待VM启动的Agent
        Running时生成VM停止子请求ID，将state置为Stopping并保存原停止请求ID，再发送StopDriverLuaVM
        Stopping且已有停止请求在途时，追加本次请求ID，不重复提交VM停止
        Stopping且上次停止已返回失败时，生成新的VM停止子请求ID并允许重试
        没有VM句柄且确定未提交或已结束执行时，清理Agent并回复成功，不伪造VM停止请求
        不因停止请求被受理就立即销毁Agent，不主动逐个清空资源集合

handle_stop_driver_lua_vm_result(world: &mut World, event: StopDriverLuaVMResult)
    处理Driver停止结果：crate公开函数
    行为：
        核对目标完整Entity、Stopping状态和在途VM停止子请求ID，旧结果或重复结果不推进当前操作
        成功结果必须保证该Agent的执行清理已经完成；保存等待回复的请求ID，销毁Agent，再逐个发送StopAgentResult::Ok(原Entity)
        停止失败时保留Agent和VM句柄，清空本次在途请求标记，向全部等待者发送DriverStopFailed错误
        失败后仍为Stopping，后续StopAgent可重试，不将还可能运行的执行体与宿主分离
        Entity销毁时五种资源集合和历史句柄自然释放，不删除历史记录或磁盘资源包
```

## 逻辑

```text
启动：
    StartAgent -> 准备信息与实体 -> Creating -> StartDriverLuaVM
    StartDriverLuaVMResult成功 -> Running -> StartAgentResult成功
    StartDriverLuaVMResult失败且执行体已清理 -> 销毁Agent -> StartAgentResult失败
    Creating信息必须在VM请求发出之前登记，避免结果先到却无法关联

停止：
    StopAgent -> Stopping -> StopDriverLuaVM
    StopDriverLuaVMResult成功 -> 销毁Agent -> StopAgentResult成功
    StopDriverLuaVMResult失败 -> 保留宿主、报告失败、允许后续重试
    销毁实体后结果中的Entity只用于实例关联，不保证还能读取组件

Driver事件完成契约：
    启动成功暂按Driver已经完成约定初始化、可服务Agent操作定义，而非仅已分配VM
    启动失败必须无本次遗留执行体，才允许Agent插件销毁实体
    停止成功必须完成Driver及它管理的活动清理，不只是接受停止请求或清除句柄
    工具、推理等在途活动如何由Driver及相应执行设施结束，需要VM和执行插件后续设计明确；不能假称只停Lua就必然全停
    Driver自然结束和运行期异常的通知契约尚待VM设计，不在此编造第三个Agent状态System

暂定异常策略：
    Creating时不支持停止取消，返回AgentBusy；需要支持时另设计启动取消与回执竞态
    超时和强制终止不在本次设计范围，不能在结果未到时先despawn
    Driver结果的完整字段由对应插件后续定义，必须保留请求关联，不能用无关联的泛型错误替代
```

# events

## 类型

公开，统一定义于共享types crate的agent_events.rs：
```text
StartAgent：Agent启动请求，公开事件
    id: String--唯一请求ID
    resource_id: ResourceID--待创建Agent的地址
    workspace: Entity--启动材料所在工作区完整Entity
    image: ResourceID--该工作区镜像集合中的镜像地址
    definition: AgentStartDefinition--成员路径、历史配置等启动参数，不含VM本体
    impl Event for StartAgent

StartAgentResult：Agent启动结果，公开事件
    id: String--原启动请求ID
    result: Result<Entity, AgentError>--成功时返回Running的Agent Entity
    impl Event for StartAgentResult

StopAgent：Agent停止请求，公开事件
    id: String--唯一请求ID
    agent: Entity--目标完整Agent Entity
    impl Event for StopAgent

StopAgentResult：Agent停止结果，公开事件
    id: String--原停止请求ID
    result: Result<Entity, AgentError>--成功时返回已清理的原Agent Entity
    impl Event for StopAgentResult
```

## 逻辑

```text
Driver VM事件由共享types中对应VM事件文件定义，Agent只使用它们
StartDriverLuaVM包含唯一子请求ID、完整Agent Entity、Driver程序和注入环境
StartDriverLuaVMResult保留子请求ID与完整Agent Entity，成功提供DriverLuaVMHandle
StopDriverLuaVM包含唯一子请求ID、完整Agent Entity及VM句柄
StopDriverLuaVMResult成功失败都必须保留子请求ID与完整Agent Entity
本次不设计VM内部调度，不沿用LuaRuntimeHandle直接注册和停止的旧控制链
公开事件及错误契约不能让共享types反向依赖Agent插件实现
删除AgentCreateRequest、AgentCreateReply、AgentControl、AgentControlReply和AgentInitializationCompleted旧入口
不通过AgentMessage转发输入，不额外发AgentCreated或通用AgentFailure事件代替对应结果
```

# types

## 类型

公开：
```text
AgentInfo：启动配置快照，公开结构体
    model: AgentImageModelConfig--从启动镜像复制的模型及采样参数
    project_root: PathBuf--启动工作目录
    home_root: PathBuf--显式主目录配置，不擅自把它等同于project_root
    image: ResourceID--启动镜像的来源地址，不是独立镜像Entity
    image_dependencies: Arc<[ResourceID]>--镜像声明的可导入地址，不表示资源已加载

AgentState：Agent生命周期，公开枚举
    Creating(AgentStartState)
    Running
    Stopping(AgentStopState)

AgentStartState：Agent启动过程，公开结构体--由state持有，不是独立Resource
    request_id: String--原StartAgent请求ID
    vm_request_id: String--正在等待的Driver启动子请求ID

AgentStopState：Agent停止过程，公开结构体--由state持有，不保存VM执行状态
    request_ids: Vec<String>--本次停止完成后需要回复的StopAgent请求ID
    vm_request_id: Option<String>--在途Driver停止子请求ID，None表示上次尝试已失败并可重试

AgentStartDefinition：成员启动参数，公开共享契约
    project_root: PathBuf--项目工作目录
    home_root: PathBuf--主目录
    history_path: PathBuf--历史表存储位置

AgentBlock：运行时Block容器，公开类型--统一承载普通Block与RefBlock，不是额外Component
    blocks: BlockAssembly--普通Block存储，私有字段
    ref_blocks: RefBlockAssembly--引用Block存储，私有字段
    说明：复用共享Block类型和已有机械操作能力，普通与引用Block共享命名空间
        类型、范围和引用合法性由容器接口维护，MCL不绕过接口任意改内部map
        不保留实时表、state_bindings或数据库同步职责；完整机械方法契约在Block领域设计中维护

AgentHistoryHandle：历史表句柄，公开可克隆类型
    inner: Arc<dyn AgentHistoryStore>--历史追加接口
    说明：具体数据库实现由历史存储领域提供，不在Agent生命周期Handler内展开SQL

AgentHistoryStore：历史追加接口，公开trait
    约束：Send + Sync + 'static
    append(&self, record: HistoryRecord) -> Result<(), HistoryError>
        追加记录：公开trait方法，只增量写入历史记录，不保存实时Block状态

HistoryRecord：历史增量记录，公开共享类型
    source: String--记录来源
    kind: String--记录类别
    content: String--正文
    payload: serde_json::Value--结构化附加数据，消息、工具信息或用量由调用方按记录协议提供
```

## 逻辑

```text
外部句柄：
    DriverLuaVMHandle由VM公共契约提供，Agent不在其中记录第二份生命周期状态
    历史句柄取得方式由存储领域后续明确；允许通过公开工厂或依赖反转接口提供，不恢复实时表接口

移除旧聚合：
    AgentCreationState、AgentInitializationState、AgentLuaState和AgentLifecycleState由新的state和lua替代
    删除AgentResourceMap、AgentResourceEntry和Agent内复制的资源内容，以五种ResourceSet替代
    Agent不再包含AgentInferenceState、AgentToolState及其pending表；相应操作的跟踪与取消职责留待所属领域设计，不等于把仍在执行的任务丢弃
    不在Agent保存AgentTurnState、TokenUsageState和last_error等旧聚合字段；需要的信息由历史记录、运行时Block或相应操作结果表达，具体消费方随后迁移
    不保存实时数据库句柄，不在Agent定义MCL转发和字段业务用途
```

# error

## 类型

公开，共享错误契约：
```text
AgentErrorKind：Agent生命周期错误分类，公开枚举
    InvalidRequest
    InvalidDefinition
    DuplicateAgent
    AgentNotAlive
    AgentBusy
    WorkspaceNotAlive
    ImageNotLoaded
    ResourceSetupFailed
    HistorySetupFailed
    DriverStartFailed
    DriverStopFailed

AgentError：Agent错误，公开结构体
    kind: AgentErrorKind--错误分类
    message: String--错误描述
    impl Clone + Debug + PartialEq + Eq for AgentError
    impl fmt::Display for AgentError
        格式化错误：输出分类与描述
    impl std::error::Error for AgentError

HistoryError：历史追加错误，公开共享错误--由历史存储契约定义，不包含实时状态错误
```

## 逻辑

```text
错误处理：
    每个合法请求在成功或失败路径都通过对应结果事件完成关联
    同步启动失败可在无执行体时回收，异步启动失败须由VM契约确保执行体已清理
    停止失败不当作停止成功，不提前销毁仍可能运行的宿主
    重复结果不重复回收，实体代数与子请求ID共同阻止旧结果命中新实例
    历史追加错误由Agent::append_history返回，不自动变为生命周期错误
    沙箱存在性校验在调用Agent::switch_sandbox前完成，未通过时原激活地址不变
```
