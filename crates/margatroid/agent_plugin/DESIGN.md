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
handler    src/handler.rs    生命周期及异常处理Handler
events     共享types crate    agent_events.rs中的全部事件
types      共享components及契约    状态机、配置、历史接口和运行时Block容器
error      共享错误契约       生命周期与历史写入错误
```

**本文是目标设计，尚未同步到Rust代码。本次只改Agent设计文档，不实现其他插件。**

# lib

## 组件定义位置

Agent图书馆组件、AgentInfo、AgentState等公开关联类型，以及WorldAgentExt的声明和World实现，计划写入共享types crate的`agent_components.rs`。对应全部事件写入各自的`*_events.rs`，不在插件实现crate重复定义。

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
    lua: AgentLua--wake槽位、ready与ended暂存槽位和Driver关闭句柄，不复制VM状态
    history: AgentHistoryHandle--历史表句柄，不含实时表或Block持久化接口
    block: AgentBlock--仅保存普通Block的运行时容器
    execution: AgentExecutionScope--共享执行作用域，管理受理、根取消及活动真实完成等待
    mcl: AgentMclQueue--目标Agent的MCL指令FIFO队列、异步队首关联及收口通知
    turn: Option<String>--当前轮次代号，不保存旧turn聚合状态
    new_turn(&mut self) -> String
        生成新轮次：公开方法
        行为：生成新的唯一轮次代号，替换turn并返回代号；不启动外部操作。
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
                挂载start_agent_system、stop_agent_system和agent_exception_system
                不注册资源类型，tool、skill、hook、prompt、sandbox分别由其资源插件注册
                不直接依赖Workspace、AgentImage或Driver VM插件的Handler和安装标记
```

## 扩展 trait

公开，声明与实现均计划位于共享types crate的agent_components.rs：
```text
WorldAgentExt：Agent受检访问扩展，公开trait--调用方只依赖共享类型层，不依赖agent_plugin实现
    agent_switch_sandbox(&mut self, agent: &ResourceID, sandbox: Option<ResourceID>) -> Result<(), AgentError>
        切换沙箱：公开方法
        行为：
            按agent地址查询唯一图书馆Entity，检查agent类型段及Agent组件
            sandbox为Some时检查sandbox类型段，并检查该Entity的ResourceSet<SandboxResource>包含此ID
            完全相同的当前配置为无修改操作，可幂等成功；同ID策略已改变不能仅按ID相等跳过检查
            在execution短锁下检查沙箱活动并更新配置；普通runner、长期进程及取消中或清理失败的Sandbox活动未释放时返回ExecutionBusy
            Inference与Other活动不阻止切换；Stopping或清理失败受理关闭时拒绝修改
            活动登记、取得沙箱策略快照和沙箱配置切换使用同一把短锁，不能先检查后另行登记；锁不跨await
            查询或检查失败返回错误，保持原激活沙箱不变
            结束查询的不可变借用后取得Agent可变引用，更新sandbox；None表示取消激活
            不加载资源，不修改已开始调用的策略快照；激活沙箱同ID策略重载也须经过活动检查
    agent_cleanup_tasks(&mut self, agent: &ResourceID) -> Result<oneshot::Receiver<Result<(), AgentError>>, AgentError>
        请求清理所属任务：公开方法
        行为：
            定位完整Agent Entity，在execution短锁下暂停Sandbox活动受理并发起清理，仅取消Sandbox活动，不取消Inference或作用域根cancel
            返回一次性清理结果Receiver，不在World借用期间或ECS主线程等待退出
            执行设施等待普通runner及长期进程退出、管道收口和资源释放，真实清理完成才mark_completed释放登记
            成功且Agent未进入Stopping时恢复Sandbox活动受理；停止关闭优先，不能被清理成功重新打开
            失败保留清理失败状态、Sandbox活动与其受理关闭，不直接清空活动列表，不删除可查询的已完成结果；Inference继续独立受理直到Stopping
            已结束且资源已清理、仅结果未被读取的任务不阻止沙箱切换
            清理后切换仍重新检查；清理与切换不是组合原子操作，受理登记与单次切换始终互斥
            清理具名输出与错误关联载荷定义在共享agent_events.rs，不调用ToolPlugin私有Handler
    agent_register_activity(&mut self, agent: &ResourceID, kind: ExecutionActivityKind) -> Result<ExecutionActivity, AgentError>
        登记活动：公开方法
        行为：受检寻址完整Entity，在execution短锁内检查对应受理门、登记活动并取得不可变沙箱策略快照；Inference与Other不依赖沙箱
            返回活动cancel及完成能力，提交失败必须撤销未启动登记；提交成功后只能在真实清理完成时mark_completed
            不在World借用内await；普通runner、长期进程、Inference及Other都登记，停止统一等待，不用Future被丢弃代替清理
    agent_operate_block(&mut self, agent: &ResourceID, operation: BlockOperation) -> Result<BlockOperationResult, AgentError>
        操作Block：公开方法
        行为：
            定位唯一Agent Entity并取得block
            调用Block容器对应的机械操作方法，保持字段类型和范围不变量
            返回类型化结果或错误，不解析MCL，不按字段名称推断业务用途
    agent_append_history_batch(&self, agent: &ResourceID, records: Vec<HistoryRecord>) -> Result<(), AgentError>
        批量追加历史：公开方法
        行为：定位唯一Agent Entity，调用history.append_batch，在同一事务中按幂等键追加全部记录；失败全部回滚并映射HistoryWriteFailed
            key包含完整Agent Entity、command id、kind及call id/index，同键同内容重试不重复追加，同键不同内容返回冲突
            指令历史事务成功才执行；执行后的结果历史失败只报告独立history_error，不重做指令或外部副作用
            只增量写历史，不覆盖旧记录、不维护实时表，也不自动停止或销毁Agent
    agent_append_history(&self, agent: &ResourceID, record: HistoryRecord) -> Result<(), AgentError>
        单条追加：公开兼容方法，委托agent_append_history_batch(agent, vec![record])，遵守同一事务与幂等契约
    impl WorldAgentExt for World
        World：公开trait实现，完整实现以上寻址、校验和数据操作，不把实现留在agent_plugin
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
    lua只提供通知槽位和关闭句柄的存储，AgentState是Agent生命周期的唯一存储
    Agent不执行ready、wake、wait或关闭操作；MCL取用通知槽位，LuaRuntime取用关闭句柄
    wake为常驻共享槽位，Driver宿主wait与MCL wake共享同一对象，不跨线程借用Agent组件
    ready暂存Sender，Receiver移入启动闭包；关闭句柄由LuaRuntime安装和使用，Agent仅保存
    block仅包含普通Block，取消RefBlock、RefMerge和持久引用关系
    mcl保存目标Agent的指令队列，不复制Block或资源内容；不同来源的请求进入同一FIFO
    Agent只保存队列，入队、推进、异步收口及MCL结果发送由MCL插件负责
    历史句柄只提供追加历史能力，不沿用带实时state读写的旧AgentMemoryHandle

扩展归属与命名：
    WorldAgentExt及其World实现都在共享types中，调用方不需要依赖Agent插件
    领域提供的World扩展方法统一采用领域前缀：agent_xxx、workspace_xxx
    历史批量追加为agent_append_history_batch，单条agent_append_history委托同一契约；沙箱切换为agent_switch_sandbox，活动登记为agent_register_activity，Block入口为agent_operate_block
    校验封装在扩展内部，不要求每个调用者自行检查沙箱集合
    方法借用World而非取得所有权，返回后调用方自然继续使用World，不用返回World
    扩展只依赖共享组件、资源查询和历史抽象接口，不调用生命周期Handler、具体数据库或MCL解析器
    资源集合仍是资源数据来源，Agent不复制工具描述、schema、正文或沙箱策略

MCL边界：
    不存在AgentControl入口，不转发MCL，不恢复长期VM邮箱投递
    MCL解析命令后调用共享WorldAgentExt的agent_operate_block，通过block容器机械接口操作字段
    Agent插件不解释Block名称和内容用途，不代替MCL做语法解析
    同Agent按队首顺序执行，异步在途命令保留队首并设置pending；其他Agent独立推进
    Driver注入wait不进入MCL队列，不占用pending；队列不是Driver邮箱或Lua调用栈
    进入Stopping时同步关闭MCL入队门和execution全部受理门，未执行排队指令逐个完成Cancelled，在途pending.cancel请求取消
    MCL在途结果/取消路径仍须完成回执及pending释放，commands为空且pending=None才发送drained通知；不只丢弃队列
    Driver最终清理同时等待execution全部活动真实完成和mcl.drained，ECS主线程不等待
```

# system

## 函数

crate公开：
```text
start_agent_system(world: &mut World)
    启动System：crate公开System
    处理事件：StartAgent、StartDriverLuaVmResult
    产出事件：StartDriverLuaVm、StartAgentResult
    行为：
        克隆本帧全部StartAgent，逐个调用handle_start_agent
        克隆本帧全部StartDriverLuaVmResult，逐个调用handle_start_driver_lua_vm_result

stop_agent_system(world: &mut World)
    停止System：crate公开System
    处理事件：StopAgent、StopDriverLuaVmResult
    产出事件：StopDriverLuaVm、StopAgentResult
    行为：
        克隆本帧全部StopAgent，逐个调用handle_stop_agent
        克隆本帧全部StopDriverLuaVmResult，逐个调用handle_stop_driver_lua_vm_result

agent_exception_system(world: &mut World)
    Agent运行期异常System：crate公开System
    处理事件：DriverLuaVmExceptionResult、AgentException
    产出事件：AgentExceptionResult
    行为：分别调用handle_driver_lua_vm_exception_result和handle_agent_exception；不消费自己的AgentExceptionResult，不把结果再包装为AgentException
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
        info保存配置，workspace保存工作区ResourceID，sandbox初始为空，lua初始化空wake槽位、空ready与ended暂存槽位和空关闭句柄，execution初始化共享开放作用域，block为空普通Block容器，mcl.commands为空VecDeque、mcl.pending为None、mcl.drained初始已收口，turn为None
        生成VM启动子请求ID，先将原StartAgent请求ID和子请求ID写入Creating
        整理注入环境，包含完整Agent Entity、Agent地址、启动配置及所需环境能力，提交StartDriverLuaVm
        提交VM前的任何失败均释放本次已创建Entity和句柄，发送关联原请求的StartAgentResult::Err
        不在发出VM请求后提前返回启动成功

handle_start_driver_lua_vm_result(world: &mut World, event: StartDriverLuaVmResult)
    处理Driver启动结果：crate公开函数
    行为：
        核对目标完整Agent Entity、Creating状态和VM启动子请求ID，旧结果或重复结果不推进其他实例
        成功时检查本次lua实例关联，确认关闭句柄已由LuaRuntime保存，将state改为Running，发送原请求ID对应的StartAgentResult::Ok(agent)
        失败时确认VM结果契约保证本次执行体已清理，保存原请求上下文，销毁Agent并发送StartAgentResult::Err
        CleanupFailed表示无法保证没有遗留活动，收到时保留宿主与关闭句柄并报告启动错误，不执行普通失败的despawn路径
        将state置Stopping且vm_request_id=None、request_ids为空，execution与MCL门保持关闭，表示启动已失败且宿主仅用于清理管理；不保留无结果的Creating等待，不自动重新启动或销毁
        每个合法受理的启动请求无论成功失败都必须有结果；无关或重复事件不重复回复

handle_stop_agent(world: &mut World, request: StopAgent)
    处理停止请求：crate公开函数
    行为：
        检查目标Entity和Agent，不存在时记录warning并发送AgentNotAlive错误
        Creating时返回AgentBusy，不抢先销毁正在等待VM启动的Agent
        Running时在同步受理操作中置Stopping，关闭MCL及所有活动受理，取消execution根token和pending.cancel，MCL完成未执行排队Cancelled；保存原请求及VM停止子请求ID，再发送StopDriverLuaVm
        Stopping且已有停止请求在途时，追加本次请求ID，不重复提交VM停止
        Stopping且上次停止已失败时保留宿主、活动及关闭受理；ended已消费时返回DriverStopFailed，不重建结束通道或虚构完成证明
        关闭句柄的缺失不能被当作执行已停止的证明，仍通知LuaRuntime核对本次执行；Agent自己不调用句柄关闭
        不因停止请求被受理就立即销毁Agent，不主动逐个清空资源集合

handle_stop_driver_lua_vm_result(world: &mut World, event: StopDriverLuaVmResult)
    处理Driver停止结果：crate公开函数
    行为：
        核对目标完整Entity、Stopping状态和在途VM停止子请求ID，旧结果或重复结果不推进当前操作
        成功结果必须保证Driver、作用域全部活动真实清理和MCL队列回执收口均完成；保存等待回复的请求ID，销毁Agent，再逐个发送StopAgentResult::Ok(原Entity)
        停止失败时保留Agent、通知存储和关闭句柄，清空本次在途请求标记，向全部等待者发送DriverStopFailed错误
        失败后仍为Stopping；不承诺已消费ended后可直接重试，不将还可能运行的执行体与宿主分离
        Entity销毁时五种资源集合和历史句柄自然释放，不删除历史记录或磁盘资源包

handle_driver_lua_vm_exception_result(world: &mut World, event: DriverLuaVmExceptionResult)
    处理Driver异常结果：crate公开函数
    行为：从event.instance.owner核对完整Agent Entity及Agent.lua.instance；使用exception_id去重，不读取不存在的event.owner或event.error
        result只表示LuaRuntime处理结果；该Result没有原error，Agent结果error为None，不增加全局异常上下文表或虚构原异常
        将result、execution_ended、cleanup_completed映射为AgentExceptionResult，并填入agent/resource_id/workspace/instance
        cleanup_completed只代表Driver已证明作用域及MCL收口；失败保留宿主、活动及关闭受理，不despawn，不再产AgentException

handle_agent_exception(world: &mut World, event: AgentException)
    处理Agent本层异常：crate公开函数
    行为：核对event.agent、resource_id、workspace及instance，按exception_id一次处理
        关闭新MCL与活动受理并请求取消；如需清理，通过既有停止/作用域等待协议异步收口，不阻塞主线程
        输出AgentExceptionResult，保留原error，result表示本层处理成功/失败，cleanup_completed仅在真实清理和MCL收口证明成立时为true
        无效关联输出对应Err且不修改其他实例；重复已处理异常不重复取消或发结果
        异常处理不自动回收宿主、不自动恢复受理；生命周期请求仍由原结果链完成，不向自己重新发送AgentException

```

## 逻辑

```text
启动：
    StartAgent -> 准备信息与实体 -> Creating -> StartDriverLuaVm
    StartDriverLuaVmResult成功 -> Running -> StartAgentResult成功
    StartDriverLuaVmResult失败且执行体已清理 -> 销毁Agent -> StartAgentResult失败
    Creating信息必须在VM请求发出之前登记，避免结果先到却无法关联

停止：
    StopAgent -> Stopping -> StopDriverLuaVm
    StopDriverLuaVmResult成功 -> 销毁Agent -> StopAgentResult成功
    StopDriverLuaVmResult失败 -> 保留宿主、活动及关闭受理，报告失败；不重复消费ended
    销毁实体后结果中的Entity只用于实例关联，不保证还能读取组件

Driver事件完成契约：
    启动成功表示ready已完成约定初始化、可服务Agent操作，而非仅已分配VM
    启动失败必须无本次遗留执行体，才允许Agent插件销毁实体
    停止成功由ended Ok证明Driver、execution全部Inference/Sandbox/Other活动真实清理及MCL drained均完成，不只是接受停止请求或清除句柄
    Driver结束无论正常、异常或取消都走最终清理，根cancel请求取消全部活动（Inference、Sandbox、Other），等待各执行/加载设施mark_completed并等待MCL回执收口
    Driver运行期异常由agent_exception_system受理，正常终态不是异常，不通过异常System替代启动或停止回复

生命周期边界：
    Creating时不支持停止取消，返回AgentBusy；需要支持时另设计启动取消与回执竞态
    超时和强制终止不在本次设计范围，不能在结果未到时先despawn
    Driver结果严格使用lua_runtime_events.rs声明的id/owner/instance/result关联，异常结果使用exception_id/instance/result及执行清理事实
```

# events

## 定义位置

所有事件无论对外业务通知还是内部调度，都定义在共享types的`agent_events.rs`，插件实现不再有`events.rs`。内部仅表示用途，不意味着在共享crate中使用pub(crate)后其他crate仍能访问；供插件使用的事件需要公开可见。

自动异步响应仍是框架的`Result<T, E>`，不另造包装事件；其中具名输出、错误及其传递所需的关联契约也在共享事件文件定义，不使共享types反向依赖插件私有类型。Plugin配置和非事件处理状态仍留在实现侧。基础设施事件如ClosureRequest沿用基础设施定义，不在业务文件复制。

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
AgentException：Agent本层运行期异常输入，公开事件
    exception_id: String--本次异常唯一ID
    agent: Entity--完整Agent实例
    resource_id: ResourceID--Agent逻辑地址
    workspace: ResourceID--所属工作区逻辑地址
    instance: DriverLuaVmInstance--异常来源VM实例
    error: AgentError--异常详情
    cleanup_completed: bool--是否已经确认执行清理完成
    impl Event for AgentException

AgentExceptionResult：Agent异常处理结果，公开事件
    exception_id: String--原异常ID，与Driver异常ID同链关联
    agent: Entity--完整Agent实例，从instance.owner或原AgentException核对
    resource_id: ResourceID--Agent逻辑地址
    workspace: ResourceID--所属工作区逻辑地址
    instance: DriverLuaVmInstance--原启动实例
    error: Option<AgentError>--本层AgentException输入保留Some(error)，Driver Result来源为None；不从不存在的字段虚构原异常
    result: Result<(), AgentError>--本层处理结果，不表示原异常不存在
    execution_ended: bool--处理后Driver是否退出
    cleanup_completed: bool--是否已证明全部活动和MCL收口
    impl Event for AgentExceptionResult

AgentTaskCleanupOutput：沙箱活动清理输出，公开共享异步载荷
    agent: Entity--完整Agent实例
    operation_id: String--清理操作关联
    result: Result<(), AgentError>--仅Sandbox活动清理结果，不代表Inference停止
```

## 逻辑

```text
Driver VM事件由共享types中对应VM事件文件定义，Agent只使用它们
StartDriverLuaVm包含唯一子请求ID、完整Agent Entity、Driver程序和注入环境
StartDriverLuaVmResult保留子请求ID与完整Agent Entity，成功表示ready已完成；关闭句柄已存于Agent.lua，不随结果再复制一份
StopDriverLuaVm包含唯一子请求ID、完整Agent Entity和启动实例关联；LuaRuntime从Agent.lua取关闭句柄，不由Agent调用或在事件里搬走句柄
StopDriverLuaVmResult成功失败都必须保留子请求ID与完整Agent Entity
本次不设计VM内部调度，不沿用LuaRuntimeHandle直接注册和停止的旧控制链
公开事件及错误契约不能让共享types反向依赖Agent插件实现
删除AgentCreateRequest、AgentCreateReply、AgentControl、AgentControlReply和AgentInitializationCompleted旧入口
不通过AgentMessage转发输入，不额外发AgentCreated或通用AgentFailure事件代替对应结果
```

# types

## 类型

公开：
```text
AgentLua：Driver访问存储，公开类型--随Agent组件保存在共享types的agent_components.rs
    instance: Option<DriverLuaVmInstance>--本次启动关联，供旧结果核对，不是VM状态
    wake: Arc<Mutex<DriverWakeSlot>>--常驻共享通知槽位，Agent创建时即存在
    ready: Option<DriverReadySlot>--本次启动oneshot发送端的暂存，启动结束后清除
    ended: Option<DriverEndedSlot>--启动时暂存结束Receiver，Stopping时由LuaRuntime取走
    close: Option<DriverCloseHandle>--跨线程关闭能力，LuaRuntime在提交Driver前安装

DriverWakeSlot：运行期等待槽位，公开共享类型--Agent只保存，操作由宿主wait、MCL wake和LuaRuntime停止流程实施
    sender: Option<oneshot::Sender<()>>--本次wait的发送端
    pending_wake: bool--wake先到时锁存一次通知，不自动产生唤醒
    closed: bool--停止后拒绝新等待，丢弃Sender使等待端观察关闭
    waiting: bool--是否已有一次wait在途，防止Sender被wake取走后第二个wait抢占
    说明：检查和修改在同一短期锁下完成，锁不跨await
        wait消费pending_wake时立即返回，否则建立oneshot并等待Receiver
        wake有Sender时取出并通知，没有Sender时锁存pending_wake
        wait完成或取消后结束在途标记，同一Driver重复并发wait拒绝
        不暴露reserve、claim和yield协议，不增加全局等待表

DriverReadySlot：启动通知暂存，公开共享类型
    instance: DriverLuaVmInstance--本次启动身份
    sender: Arc<Mutex<Option<oneshot::Sender<Result<(), LuaRuntimeError>>>>>--共享同一个发送端存储
    说明：MCL ready和执行失败路径在锁内take Sender，只有一方能完成本次启动裁决
        Receiver由启动闭包持有，Agent不持有接收端，不另存Pending/Ready/Failed状态机
        使用Arc便于执行任务在ready前失败时交付错误，不引入注册表或第二份Sender
        Driver可调用ready之前必须安装本次槽位；MCL ready同步发送，不等待启动结果事件
        Sender已被取走或槽位已清除时返回无需重复发送的信息，不宣称ECS已进入Running
        启动结果收集后移除Agent.lua.ready，旧任务不得取得新实例槽位

DriverEndedSlot：结束接收端暂存，公开共享类型
    instance: DriverLuaVmInstance--本次启动身份
    receiver: oneshot::Receiver<Result<(), LuaRuntimeError>>--仅由停止闭包一次性取走；Ok证明Driver、全部作用域活动及MCL收口完成
    说明：发送端移入Driver任务，任务结束并清理后发送；提前发送也可由后续停止等待收到
        Agent仅保存，不接收信号、不主动关闭；停止等待取走后不在原字段建立替代通道

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
    vm_request_id: Option<String>--在途Driver停止子请求ID，None表示本次等待已结束；ended已消费时不代表可以直接重试

AgentStartDefinition：成员启动参数，公开共享契约
    project_root: PathBuf--项目工作目录
    home_root: PathBuf--主目录
    history_path: PathBuf--历史表存储位置

AgentMclQueue：目标Agent指令队列，公开类型--随Agent定义于共享agent_components.rs，不是独立Component或Resource
    commands: VecDeque<MclCommand>--受理顺序排列，异步在途指令仍保留在队首
    pending: Option<MclPending>--当前异步队首关联，空值表示可继续推进
    drained: AgentMclDrainHandle--线程安全收口通知，入队时置未收口，队列与pending均空且回执完成时通知；Driver可捕获并异步等待
    accepting: bool--主线程MCL入队门；同时检查execution整体受理门，Stopping或Driver request_close后置false，不允许该标志绕过共享关闭门
    说明：创建Agent时队列为空，pending为None；MclCommand定义于共享mcl_events.rs
        只有匹配本次目标完整Entity、操作关联和队首的异步结果才能释放pending并移除队首
        不保存第二份全局pending表，不负责执行命令或解释语法

MclPending：异步队首关联，公开类型--随队列定义于共享agent_components.rs
    command_id: String--在途队首请求ID
    target_entity: Entity--提交时的完整目标Entity，包含代数
    operation_id: String--本次异步操作唯一关联，防止迟到或重复结果命中另一操作
    cancel: CancellationToken--在途指令取消能力，Stopping时请求取消，结果路径仍完成回执

AgentBlock：运行时Block容器，公开类型--仅承载普通Block，不是额外Component
    blocks: BlockAssembly--普通Block存储，私有字段
    说明：复用共享Block字段类型及范围检查，不保留RefBlockAssembly、RefBlock或RefMerge
        类型和范围合法性由容器接口维护，MCL不绕过接口任意改内部map
        多选择器读取由MCL按顺序取值并拼接快照，不建立引用，也不新增MERGE机械操作
        不保留实时表、state_bindings或数据库同步职责；完整机械方法契约在Block领域设计中维护

BlockOperation与BlockOperationResult：共享Block领域的类型化操作和结果契约
    说明：供agent_operate_block使用，具体变体随Block领域设计确定，不把任意JSON当作机械操作

AgentExecutionScope：Agent执行作用域，公开可克隆共享句柄--定义及实现位于共享agent_components.rs
    inner: Arc<Mutex<AgentExecutionData>>--短锁维护受理门、根cancel、活动登记和完成等待通知，不跨await
    request_close(&self)
        关闭作用域：短锁关闭全部受理门并请求root_cancel，Stopping及Driver最终清理调用，幂等且不可重新打开
    wait_all(&self) -> impl Future<Output = Result<(), AgentError>>
        等待全部活动：完成通知异步等待所有登记真实清理；清理失败返回Err并保留未清理登记，不把取消请求当完成
    register_with_snapshot<F>(&self, kind: ExecutionActivityKind, snapshot: F) -> Result<ExecutionActivity, AgentError>
        受检登记：公开方法，执行设施通过World入口调用
        约束：F: FnOnce() -> Result<Option<SandboxPolicySnapshot>, AgentError>
        行为：在同一短锁内检查受理门，调用只读World快照回调并登记活动；回调不能await、重入作用域或调用可变World操作，失败不登记；沙箱更新也取得同锁。
    说明：活动登记与沙箱快照取得/切换互斥；Inference独立于沙箱，Sandbox含runner和长期进程
        停止关闭全部受理并取消根token；单独agent_cleanup_tasks只暂停/取消Sandbox，不取消Inference、Other或根token
        实际活动由执行/加载设施登记一次；MCL pending仅管理逻辑队首等待，不重复登记同一Inference、runner或IMPORT活动
        wait_all异步等待全部活动真实清理；取消中、清理失败仍登记，不能因结果已发或Future丢弃释放
        提交失败撤销未启动登记；mark_completed只用于真实清理完成，结果未读但已清理不阻止切换

AgentExecutionData：执行作用域内部数据，公开共享内部契约
    admission: ExecutionAdmission--Open、SandboxPaused、CleanupFailed或Closed；Closed为停止终态，不能被单清理成功重开
    root_cancel: CancellationToken--停止/最终清理取消Inference、Sandbox及Other全部活动
    activities: Map<ActivityID, ExecutionActivityState>--活动类别、子cancel及真实清理事实，非业务结果仓库
    completion: SharedCompletionNotify--登记和订阅在短锁内完成，异步等待重查以避免丢唤醒

SandboxPolicySnapshot：沙箱策略快照，公开共享结构体
    resource_id: ResourceID--启动时激活策略身份
    policy: Arc<str>--启动时不可变内容，无激活沙箱则活动快照为None

ExecutionActivityState：活动登记状态，公开共享内部结构体
    kind: ExecutionActivityKind
    cancel: CancellationToken
    cleanup_error: Option<AgentError>--失败保留登记并使等待返回Err
    说明：真实清理完成即移除登记，活动结果和长期任务可查询输出不存于此表。

ExecutionActivityKind：执行活动类别，公开枚举
    Inference--停止时取消并等待，不阻止沙箱切换，不被agent_cleanup_tasks取消
    Sandbox--普通runner或长期进程，真实清理前阻止沙箱切换
    Other--skill、IMPORT及一般无沙箱异步活动；停止取消并等待，不阻止沙箱切换，不被单独Sandbox清理取消

ExecutionActivity：活动登记句柄，公开共享类型
    id: ActivityID--作用域内唯一关联
    kind: ExecutionActivityKind--活动类别
    cancel: CancellationToken--根token的子取消能力，单Sandbox清理只取消相应子token
    sandbox_snapshot: Option<SandboxPolicySnapshot>--同登记锁内取得的不可变策略快照，Inference及Other为None
    scope: Weak<Mutex<AgentExecutionData>>--私有回连，mark_completed及rollback据此更新活动表并通知等待者，宿主保留到清理完成
    mark_completed(&self) -> Result<(), AgentError>
        完成登记：真实退出、管道收口及资源释放后一次性释放登记并通知等待者；重复完成幂等，Drop不等于清理
    rollback_submission(&self) -> Result<(), AgentError>
        撤销登记：仅提交失败且执行体未启动可用，已启动不可借此释放
    mark_cleanup_failed(&self, error: AgentError)
        标记清理失败：公开方法
        行为：短锁保留活动登记与错误事实并通知等待者，使wait_all返回关联Err而非永久等待；不得作为mark_completed调用。

AgentMclDrainHandle：队列收口等待共享句柄，公开类型--定义及实现位于agent_components.rs
    说明：短锁维护MCL受理门、停止请求及收口等待，不跨await；通知只证明commands和pending清空且所有已受理请求回执完成
    close(&self)
        关闭MCL受理：短锁关门并请求pending关联取消，幂等；不跨线程直接修改VecDeque
    wait_drained(&self) -> impl Future<Output = Result<(), AgentError>>
        等待回执收口：异步等待共享通知，不阻塞主线程
    说明：Driver最终清理先关闭execution全门并close；MCL command System第三阶段每帧检查execution整体受理门closed
        主线程逐项完成未执行排队Cancelled并推进pending结果/取消回执，全部收口后通知wait_drained；不新增MCL停止事件或执行结束事件

AgentHistoryHandle：历史表句柄，公开可克隆类型
    inner: Arc<dyn AgentHistoryStore>--历史追加接口
    说明：具体数据库实现由历史存储领域提供，不在Agent生命周期Handler内展开SQL

AgentHistoryStore：历史追加接口，公开trait
    约束：Send + Sync + 'static
    append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError>
        批量追加：公开trait方法，一次事务全部提交或全部回滚；按HistoryRecord.key幂等去重，同键不同内容报冲突，不保存实时Block状态

HistoryRecord：历史增量记录，公开共享类型
    key: String--编码完整Entity、command_id、kind及call_id/index的稳定幂等键，异步结果沿用提交关联
    source: String--记录来源
    kind: String--记录类别
    content: String--正文
    payload: serde_json::Value--结构化附加数据，消息、工具信息或用量由调用方按记录协议提供
    turn: Option<String>--该记录所属轮次快照，不在异步结果到达时重新读取当前turn
```

## 逻辑

```text
外部句柄：
    DriverCloseHandle由共享VM契约提供，Agent只存储，LuaRuntime负责安装与请求取消，停止闭包通过ended取得清理结果
    不提供Agent::ready、Agent::wake或Agent::close方法，不把这些动作加入WorldAgentExt
    历史句柄取得方式由存储领域后续明确；允许通过公开工厂或依赖反转接口提供，不恢复实时表接口

移除旧聚合：
    AgentCreationState、AgentInitializationState、AgentLuaState和AgentLifecycleState由新的state和lua替代
    删除AgentResourceMap、AgentResourceEntry和Agent内复制的资源内容，以五种ResourceSet替代
    Agent不再包含AgentInferenceState、AgentToolState及其pending表；执行结果由所属领域管理，公共取消、沙箱互斥及真实完成等待统一在execution，不丢弃在执行任务
    不在Agent保存AgentTurnState、TokenUsageState和last_error等旧聚合字段；只保留turn代号，start_turn生成并替换，finish清空
    自动历史由MCL负责：除history_append外每条指令先批量事务记录成功才执行；inference追加assistant，tool_call按call id/index逐项记录结果含失败
    history_append自身采用同一批量事务接口，执行后历史失败单独history_error，不重跑已执行指令
    AgentMessage是共享types消息值（inject/user/assistant/tool_call/error），不实现旧路由Event；inject(Vec<AgentMessage>)递归同类型，不设置内部白名单
    tool_call的resource_id为Option<ResourceID>，无匹配工具为None，不虚构资源地址；其余正文、usage及错误字段遵循共享消息值契约
    history_append接受AgentMessage数组，保留assistant usage和tool_call关联/失败详情；旧同名AgentMessage路由事件不继续
    不保存实时数据库句柄，不在Agent定义MCL转发和字段业务用途
```

# error

## 类型

公开，共享错误契约：
```text
AgentErrorKind：Agent生命周期及受检访问错误分类，公开枚举
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
    ResourceResolutionFailed
    InvalidResourceID
    SandboxNotLoaded
    ExecutionBusy
    TaskCleanupFailed
    BlockOperationFailed
    HistoryWriteFailed

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
    agent_append_history_batch及其单条委托将底层历史错误映射为HistoryWriteFailed返回，不自动终止生命周期
    agent_switch_sandbox内部完成类型与挂载存在性检查，未通过时原激活地址不变
```
