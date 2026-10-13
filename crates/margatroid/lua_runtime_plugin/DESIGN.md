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
lib        src/lib.rs        Plugin与执行职责
system     src/system.rs     启动、停止及运行期异常事件调度
handler    src/handler.rs    提交任务、安装槽位和使用关闭句柄
events     共享types crate    lua_runtime_events.rs中的全部事件及异步输出契约
types      共享契约及内部类型  实例关联、关闭能力、程序和环境
error      共享错误契约       执行、通知与清理错误
```

**本文说明已迁移的Driver执行实现与共享契约，保留lib、system、handler、events、types、error六板块。源码与具名API以下述当前实现说明为准；不把取消请求、任务丢弃或通知通道关闭当作清理成功。**

不再设置Lua注册表。Agent.lua保存常驻wake槽位、ready发送端暂存、ended接收端暂存及关闭句柄。
Agent只提供存储并发送生命周期请求，MCL使用通知槽位，LuaRuntime安装并使用关闭句柄。

共享源码实际归属：`lua_runtime_components.rs`定义Instance、AgentLua、通知槽、关闭句柄与错误；`lua_environment.rs`定义程序、环境、Lua值与宿主函数；`lua_runtime_events.rs`定义六种公开事件及具名异步输出。插件只保存配置与计划，不持VM注册表、owner邮箱或provider注册表。环境由调用方整理后随StartDriverLuaVm提交。

默认限制为源码1 MiB、Lua内存64 MiB、每1000条Lua指令检查协作取消，startup_timeout默认None；with_config拒绝零限制或零超时。Full库不是隔离沙箱；模块require只解析本次注入模块。wait与mcl_wait捕获共享wake槽位并异步等待；没有reserve/claim/yield或宿主block_on桥接。

# lib

## 类型

公开：
```text
LuaRuntimePlugin：Driver执行插件，公开结构体
    schedule: String--请求和启动闭包结果处理计划
    config: LuaRuntimeConfig--Lua执行限制
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法，保存计划名并返回自身
    with_config(mut self, config: LuaRuntimeConfig) -> Result<Self, LuaRuntimeError>
        设置配置：公开方法，检查限制有效后保存
    impl Default for LuaRuntimePlugin
        default() -> Self
            默认构造：使用RuntimePlugin::UPDATE与默认配置
    impl Plugin for LuaRuntimePlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                要求计划、Runtime、AsyncRuntime和Closure基础设施可用，拒绝重复安装
                要求本计划已安装ClosureSystem，保存运行时配置
                挂载start_driver_lua_vm_system、stop_driver_lua_vm_system、driver_lua_vm_exception_system
                不插入Driver注册表，不安装独立owner等待表，不再提供notify_driver_ready的注册表访问接口
                使用共享Agent组件契约，不依赖Agent或MCL插件Handler与安装标记
```

## 逻辑

```text
职责：
    Workspace创建并拥有执行线程，LuaRuntime向指定工作区提交Driver本地任务，不为每个VM开线程
    LuaRuntime创建Lua、安装环境、执行和清理Driver，不管理Agent生命周期
    主线程可读取Agent.lua并克隆execution、mcl.drained及线程安全通知句柄；执行任务只使用捕获的共享对象，不借用World
    Agent的Creating/Running/Stopping仍由Agent插件处理，LuaRuntime不改变它们
    Agent.lua的通知与关闭存储由共享types定义，源码目标为agent_components.rs及其引用的VM共享契约
    真正Lua状态与本地Future仅存在于工作区线程，不进入共享组件
```

# system

## 函数

crate公开：
```text
start_driver_lua_vm_system(world: &mut World)
    Driver启动System：crate公开System
    处理事件：StartDriverLuaVm、Result<DriverLuaVmStartOutput, DriverLuaVmStartTaskError>
    产出事件：ClosureRequest、StartDriverLuaVmResult
    行为：
        克隆本帧全部StartDriverLuaVm，逐个调用handle_start_driver_lua_vm
        读取启动闭包结果，逐个调用handle_driver_lua_vm_start_result

stop_driver_lua_vm_system(world: &mut World)
    Driver停止System：crate公开System
    处理事件：StopDriverLuaVm、Result<DriverLuaVmStopOutput, DriverLuaVmStopTaskError>
    产出事件：ClosureRequest、StopDriverLuaVmResult
    行为：
        克隆本帧全部StopDriverLuaVm，逐个调用handle_stop_driver_lua_vm
        读取停止闭包结果，逐个调用handle_driver_lua_vm_stop_result

driver_lua_vm_exception_system(world: &mut World)
    Driver运行期异常System：crate公开System
    处理事件：DriverLuaVmException、Result<DriverLuaVmExceptionOutput, DriverLuaVmExceptionTaskError>
    产出事件：ClosureRequest、DriverLuaVmExceptionResult
    行为：
        克隆本帧异常调用handle_driver_lua_vm_exception
        当前异常Handler同步核对、取消并回复，不创建异常协调闭包；共享异常异步输出保留为契约但不是已安装处理链，不消费自身公开结果
```

## 逻辑

```text
事件通道：
    启动闭包等待ready Receiver，返回后自动Pending升变为Result<DriverLuaVmStartOutput, DriverLuaVmStartTaskError>
    启动结果Handler发公开启动结果，不能把异步Result当作裸输出或公开结果
    停止闭包持有ended Receiver，返回后Pending升变为Result<DriverLuaVmStopOutput, DriverLuaVmStopTaskError>
    停止结果Handler发送公开StopDriverLuaVmResult，正常结束不额外发送结束事件
    Driver任务发生异常时通过RuntimeEventSender直接发送DriverLuaVmException，无论执行是否已经结束
    异常System处理后发送DriverLuaVmExceptionResult，不替代启动或停止请求结果
    无事件时不扫描Agent或注册表判断是否结束
```

# handler

## 函数

crate公开：
```text
handle_start_driver_lua_vm(world: &mut World, request: StartDriverLuaVm)
    处理启动请求：crate公开函数
    行为：
        校验请求ID、完整Agent Entity、Creating操作关联、源码大小及环境配置
        要求Agent.lua.close为空且没有不同启动实例正在使用这些槽位，冲突返回错误
        以完整owner Entity和启动请求ID组成DriverLuaVmInstance，不另建vm_id到实体的映射
        建立oneshot<Result<(), LuaRuntimeError>>，Receiver移动进启动闭包，Sender存入Agent.lua.ready
        同时建立ended oneshot，将Receiver暂存Agent.lua.ended，Sender移入Driver任务；创建DriverCloseHandle，保存到Agent.lua.close；填入Agent.lua.instance，克隆其常驻wake槽位供Driver宿主函数使用
        同时捕获Agent.execution和Agent.mcl.drained供最终清理使用；上述存储必须在任务可执行之前完成，提交失败撤销本次安装及未启动活动登记，不修改别的实例
        构造Send任务创建闭包，向request.execution_entry提交；在工作区线程中创建Lua与Local Future，再spawn_local运行
        Lua执行使用异步宿主调用，不在工作区线程block_on等待；保留Lua指令取消检查
        启动协调闭包只await ready Receiver，收到Ok表示初始化就绪，收到Err表示本次初始化失败
        ready等待超时先请求close取消，随后继续等待同一ready Receiver；监督任务最终清理后通过尚未消费的ready Sender返回Err，不额外抢走ended
        当前超时不直接合成StartupTimeout裁决；最终返回同一通道交付的执行或清理错误，ready若已先行裁决则不改写它
        ready Receiver异常关闭时返回CleanupFailed并保留宿主、close和原ended，不能以丢Sender证明清理完成；普通错误只能在已有ready Err清理事实下回收
        通过send_async_closure提交启动协调闭包，不关闭Runtime下一帧门，不创建独立Driver线程
        工作区本地执行Future捕获panic并先释放Lua，再将执行结果交付execution_done oneshot
        独立spawn_async_service监督任务持execution_done Receiver、共享ready Sender存储与ended Sender；不借用World、不依赖工作区任务继续存在来等待外部活动清理
        监督任务收到执行完成后关闭scope、MCL与wake，使用join等待wait_all和wait_drained；两者都完成且没有清理错误才send ended Ok
        任务在ready前发生普通错误、panic或提前返回时，监督任务在上述最终清理后尝试从同一ready Sender存储take并发送Err；清理失败明确CleanupFailed
        execution_done通道异常关闭表示缺少Driver退出证明，监督任务仍取消并等待活动/MCL，最后返回CleanupFailed，不将Future被丢弃当作真实清理
        任务在ready后正常或异常结束时不再改写启动裁决

handle_driver_lua_vm_start_result(world: &mut World, result: Result<DriverLuaVmStartOutput, DriverLuaVmStartTaskError>)
    处理启动闭包结果：crate公开函数
    行为：
        取得原请求与实例，核对Agent.lua.instance和Creating中的启动子请求ID
        成功时移除本次ready暂存，保留wake与close，发送StartDriverLuaVmResult::Ok(())
        失败时清除本次ready暂存；确认没有遗留执行体或清理已完成后发送关联启动错误
        失败且未能清理时保留关闭句柄并明确返回CleanupFailed，不能让Agent把一般失败当作可直接销毁的证明
        无关或重复结果不推进其他实例，合法启动请求只回复一次
        通用AsyncTaskError不包含请求ID；实际CoordinationGuard捕获原请求、关闭句柄及共享上下文
        启动或停止协调闭包/Future被丢弃时，Guard请求关闭并直接发送关联CleanupFailed公开结果；正常具名输出完成后disarm
        From<AsyncTaskError>仅生成无关联监督错误，System不据此虚构实例或请求ID；关联失败回执由Guard负责

handle_stop_driver_lua_vm(world: &mut World, request: StopDriverLuaVm)
    处理停止请求：crate公开函数
    行为：
        从完整owner Entity的Agent.lua读取close，核对启动实例
        take本次ended Receiver，移入停止异步闭包；不在每次Stop时重建通道
        Receiver已被取走时拒绝重复停止等待，返回StopAlreadyWaiting，不覆盖在途等待
        克隆关闭句柄请求取消，关闭共享wake槽位；核对Agent已关闭MCL与活动受理并请求根取消，不在Agent插件调用Driver关闭句柄
        停止闭包await Receiver，收到Result<(), LuaRuntimeError>后返回带原请求和实例的DriverLuaVmStopOutput
        接收端异常关闭转换为明确错误，不把Sender丢弃当作已完成清理
        通过send_async_closure提交等待，不阻塞主线程，不join或关闭工作区线程

handle_driver_lua_vm_stop_result(world: &mut World, result: Result<DriverLuaVmStopOutput, DriverLuaVmStopTaskError>)
    处理停止闭包结果：crate公开函数
    行为：
        核对完整Agent实例和原停止子请求ID，旧结果不命中新实例
        已收到ended的Ok时发送StopDriverLuaVmResult::Ok，证明Driver、execution全部活动真实清理及MCL已受理请求回执收口
        ended错误或等待任务错误发送关联Err，保留仍需管理的Agent和关闭句柄
        不销毁Agent，Agent插件在成功结果后完成实体清理
        Receiver已消费或被丢弃后返回明确错误，保留关闭句柄、宿主及活动，不重建ended、不宣称可直接重试

异常具名异步输出：共享保留契约，当前未安装对应异步结果Handler
    说明：实际handle_driver_lua_vm_exception同步核对当前实例并发送一次DriverLuaVmExceptionResult；不抢ended、不产生新的Exception，不宣称已有独立异步异常协调执行链。

handle_driver_lua_vm_exception(world: &mut World, event: DriverLuaVmException)
    处理运行期异常：crate公开函数
    行为：
        核对完整owner与启动实例，旧或重复异常不作用到新实例
        对本次异常请求Driver协作取消并关闭scope/MCL门，不抢走停止闭包独占的ended Receiver
        事件已证明execution_ended和cleanup_completed时同步确认；否则回复已请求取消且两个完成事实为false
        任务最终清理通过原ended向停止链交付，不用异常处理再等待Driver或创建第二完成通道
        若其他运行时处理确需异步，仅返回DriverLuaVmExceptionOutput并由异常结果Handler完成，不等待或竞争ended
        发送关联exception_id的DriverLuaVmExceptionResult，包含处理结果及处理后的执行和清理事实
        不修改Agent.state，不直接销毁Agent或Workspace
        result为处理成功/失败，原异常error不在DriverLuaVmExceptionResult字段中；不得让上层从result Ok推断原异常消失
        处理完成前不报告cleanup_completed为true，失败保留关闭能力、宿主及活动，受理保持关闭；不消费自己的ExceptionResult，不再次产Exception形成循环

```

## 逻辑

```text
ready：
    LuaRuntime受理启动时建立通道并把发送端暂存Agent.lua.ready，接收端由启动闭包独占
    MCL ready根据目标Agent地址定位，核对宿主上下文的完整Entity和启动ID，取出同一Sender并send(Ok(()))
    ready前失败路径也从同一共享存储take Sender，只有一方能取得，先取得者决定本次启动结果
    Sender已经取出时不能把重复ready当成新成功；不设置第二份DriverStartupPhase
    Sender先发送、Receiver后await仍可收到，不需要ready的pending_wake或全局ready表
    ready仅完成启动等待，不结束Driver；MCL不得等待启动结果事件再返回ready宿主调用

wait与wake：
    Agent.lua.wake在Agent创建时就存在，wait宿主函数捕获Arc而非跨线程借用Agent
    wait锁内检查closed、在途等待和pending_wake；有通知则消费并返回，无通知则建立oneshot并存Sender
    锁在await Receiver之前释放，不支持同一Driver两个并发wait覆盖彼此
    MCL wake按目标Agent寻址并核对完整实例，有Sender时take后发送()，无Sender时设置pending_wake
    pending_wake只记已发生的wake，多次提前通知合并，不自动产生唤醒，业务数据仍存在Block里
    LuaRuntime停止时关闭槽位并取消执行，wait必须能结束等待；不保留reserve/claim/yield协议

ended协议：
    通道在启动时创建，独立监督service持ended Sender，Agent.lua.ended暂存Receiver；工作区Driver任务只持execution_done Sender
    Driver正常返回、异常、panic或取消都进入最终清理：关闭活动/MCL受理、请求根cancel及在途MCL取消，调用mcl.drained.close()；MCL command System第三阶段每帧检查execution整体受理门closed并收口未执行排队Cancelled，不新增MCL停止通知或执行结束事件
    最终清理等待execution.wait_all中全部Inference/Sandbox/Other真实完成，并等待mcl.drained确认全部已受理请求回执收口；所有等待在异步设施中，不跨World借用
    只有Driver自身退出与资源释放、作用域全部活动真实清理、MCL收口均成功才send(Ok(()))；失败send(Err(CleanupFailed))，原始运行错误独立经异常链报告
    runner/长期进程在退出、管道收口与资源释放后才mark_completed，推理在实际结束/取消收口后完成；结果未读但已清理无需等待读结果
    Driver先结束、停止后取Receiver仍能收到结果，不丢提前结束信号
    Receiver只由一次停止闭包取走，Agent自己不await，不额外建立完成结果注册表
    正常结束不是异常事件；长期Driver非预期返回可按协议报告Exception，但仍独立完成ended
    等待闭包取消或ended错误后，原通道不可重新等待，不承诺任意失败后能重试
    丢Future不保证派生进程清理，Lua检查点和宿主取消清理仍必须落实

```

# events

## 定义位置

所有业务和内部事件，以及自动异步Result需要的共享输出契约，定义在共享types的lua_runtime_events.rs。
插件实现不保留events.rs。内部是用途标记，跨crate使用的类型必须公开可见。

## 类型

公开：
```text
StartDriverLuaVm：Driver启动请求，公开事件
    id: String--Agent生成的唯一启动子请求ID
    owner: Entity--完整Agent Entity
    program: LuaProgram--Driver源码与标准库配置
    environment: LuaEnvironment--已整理注入环境
    execution_entry: WorkspaceExecutionEntry--所属工作区的启动授权或Running受检入口
    impl Event for StartDriverLuaVm

StartDriverLuaVmResult：Driver启动结果，公开事件
    id: String--原请求ID
    owner: Entity--完整Agent Entity
    result: Result<(), LuaRuntimeError>--ready已完成或启动失败，关闭句柄已存Agent.lua
    impl Event for StartDriverLuaVmResult

StopDriverLuaVm：Driver停止请求，公开事件
    id: String--唯一停止子请求ID
    owner: Entity--完整Agent Entity
    instance: DriverLuaVmInstance--期望操作的本次启动实例，不携带关闭句柄
    impl Event for StopDriverLuaVm

StopDriverLuaVmResult：Driver停止结果，公开事件
    id: String--原请求ID
    owner: Entity--完整Agent Entity
    instance: DriverLuaVmInstance--原启动实例
    result: Result<(), LuaRuntimeError>--成功证明Driver、全部作用域活动及MCL回执收口完成
    impl Event for StopDriverLuaVmResult

DriverLuaVmException：Driver异常通知，公开事件
    exception_id: String--本次异常唯一ID，原异常error仅在本事件，Result不复制该字段
    instance: DriverLuaVmInstance--完整启动实例
    error: LuaRuntimeError--异常描述
    execution_ended: bool--异常发生时执行是否已经结束
    cleanup_completed: bool--异常发生时清理是否完成
    impl Event for DriverLuaVmException

DriverLuaVmExceptionResult：Driver异常处理结果，公开事件
    exception_id: String--原异常ID
    instance: DriverLuaVmInstance--原启动实例
    result: Result<(), LuaRuntimeError>--异常处理成功或失败，不表示原异常不存在；本事件没有error和owner字段，owner从instance.owner取得
    execution_ended: bool--处理后执行是否结束
    cleanup_completed: bool--处理后清理是否完成
    impl Event for DriverLuaVmExceptionResult

```

## 逻辑

```text
正常执行清理结果通过ended传递，不定义独立执行结束、停止完成或Finished事件
异常由执行任务直接发Exception，异常System独占该异常处理职责并发ExceptionResult
六种具名事件为start/result、stop/result、exception/result；自动异步Result通道另算
启动前失败和主动停止失败仍回复原请求，异常结果不代替生命周期结果

```

## 具名异步载荷

公开，lua_runtime_events.rs中的异步契约：
```text
DriverLuaVmStartOutput：启动等待输出，公开共享类型
    request: StartDriverLuaVm--原请求上下文
    instance: DriverLuaVmInstance--本次实例
    result: Result<(), LuaRuntimeError>--ready成功或失败

DriverLuaVmStopOutput：停止等待输出，公开共享类型
    request: StopDriverLuaVm--原停止请求
    instance: DriverLuaVmInstance--原启动实例
    result: Result<(), LuaRuntimeError>--ended交付的全作用域及MCL收口证明

DriverLuaVmExceptionOutput：异常处理异步输出，公开共享载荷--定义于lua_runtime_events.rs
    event: DriverLuaVmException--原异常及其关联，保留原error但不添加到公开Result字段
    result: Result<(), LuaRuntimeError>--处理成功或失败
    execution_ended: bool--已验证执行事实
    cleanup_completed: bool--已验证全作用域及MCL收口事实

DriverLuaVmExceptionTaskError：共享异常监督错误契约，公开类型
    event: Option<DriverLuaVmException>--可选原异常关联
    source: AsyncTaskError
    说明：当前异常Handler同步确认并发送公开结果，不启动第二个ended等待或使用该异步输出链；From不构造关联。

DriverLuaVmStopTaskError：停止协调任务错误，公开共享类型
    request: Option<StopDriverLuaVm>--可选原请求
    source: AsyncTaskError
    说明：From产生request=None；协调Guard独立保证关联失败回执，Receiver丢失不可恢复。

DriverLuaVmStartTaskError：启动协调任务错误，公开共享类型
    request: Option<StartDriverLuaVm>--可选原请求
    instance: Option<DriverLuaVmInstance>--可选本次实例
    source: AsyncTaskError
    说明：From产生两个None；协调Guard独立保证关闭及关联失败回复，不依赖全局关联表。
```


# types

## 类型

公开，组件及执行能力契约定义与实现位于共享components（Agent存储在agent_components.rs，VM能力在lua_runtime_components.rs）；Plugin配置仍在插件侧：
```text
DriverLuaVmInstance：Driver启动身份，公开类型
    owner: Entity--完整Agent Entity
    start_request_id: String--唯一启动子请求ID
    说明：不另设vm_id索引，一个Agent当前只能对应一个Driver；同Entity更换Driver也需新启动ID

DriverCloseHandle：跨线程关闭句柄，公开可克隆类型--Agent保存，LuaRuntime操作
    inner: Arc<Mutex<DriverCloseData>>--共享取消存储，不包含Lua VM或本地任务引用

DriverCloseData：Driver执行控制数据，公开共享内部类型
    instance: DriverLuaVmInstance--关联身份
    cancellation: CancellationToken--协作停止信号
    stop_requested: bool--主动停止已提出，不是Agent生命周期状态机
    说明：不保存第二份ended结果或停止等待请求表，实际清理通过ended通道报告

LuaProgram：Lua程序，公开类型
    source: Arc<str>--源码
    origin: Arc<PathBuf>--来源路径
    libraries: LuaStandardLibraries--库权限

LuaStandardLibraries：标准库选择，公开枚举
    Minimal--基础语言库并裁剪宿主访问入口
    Safe--mlua ALL_SAFE，不等于进程隔离
    Full--mlua ALL，不等于沙箱

LuaEnvironment：环境描述，公开类型
    globals: Vec<LuaGlobalBinding>--注入值与宿主函数
    modules: Vec<LuaModuleBinding>--受控模块
    说明：宿主上下文携带完整实例、共享wake、AgentExecutionScope与AgentMclDrainHandle；只读配置需实际保护
        wait从Agent提供的共享槽位建立Receiver，不通过全局map
        共享LuaValue支持Nil、Boolean、Integer、Number、String、Array与Object；环境已整理，不设provider注册表
        LuaProgram没有entry字段，执行整个chunk的exec_async；实际origin为Arc<PathBuf>
        LuaEnvironmentContext携带instance、values、wake、execution与mcl；LuaHostFunction.call(arguments, context, CancellationToken)返回Send HostFuture
        globals可绑定值或Arc<dyn LuaHostFunction>，modules通过本次注入require读取；宿主使用create_async_function，不建立block_on桥接
        宿主输出的LuaValue::Array（包括嵌套空数组）在Lua table metatable登记__dsh_array=true；返回宿主时读取该标记，空数组仍还原Array(empty)，不把Assistant.tool_calls=[]变成Object(empty)。标记不是消息字段或授权能力，Lua可修改它。
        普通未标记Lua空table仍保留Object(empty)，manual参数数组的空表解释由MCL已知数组上下文处理，不按role或字段名猜测；标记数组的混合键和稀疏索引仍拒绝。
        GET/推理结果可原样重新作为INJECT或history_append输入，Driver不需要删除tool_calls等合法字段来绕过数组桥接；此桥接不负责MCL语法或持久ID resolve。

LuaRuntimeConfig：执行限制，公开类型
    max_source_bytes: usize--源码大小上限
    max_memory_bytes: usize--内存上限
    instruction_check_interval: u32--取消检查间隔
    startup_timeout: Option<Duration>--可选ready等待上限，到期请求关闭并等实际清理，不直接释放宿主
```


## 逻辑

```text
删除旧设计：
    删除DriverLuaVmRegistry、DriverLuaVmRecord、DriverStartupPhase和DriverLuaVmStatus
    删除纯查询DriverLuaVMHandle、独立ready/wake表及重复的完成结果存储
    不把上述注册表换名放进其他Resource，也不复制通知槽位

槽位与能力归属：
    AgentLua、DriverWakeSlot、DriverReadySlot与DriverEndedSlot实际定义在共享lua_runtime_components.rs，由Agent组件持有
    AgentLua.handled_exceptions保存当前实例已处理异常ID，启动安装时清空，异常Handler按实例去重，不新增全局表
    Agent仅提供存储，不提供或调用ready、wake、wait、close业务方法
    MCL取用ready Sender与wake共享槽位；LuaRuntime安装并取用close句柄
    执行任务可以捕获同一个关闭数据和ready Sender存储的Arc，处理失败或更新结束结果，不跨线程访问World
    主线程只取槽位和发取消，ended由停止异步闭包等待，不在ECS线程阻塞
    关闭句柄跨线程必须Send和Sync，真实Lua、本地Future或LocalSet的任务对象不能装入它

工作区执行：
    一个工作区拥有一条线程及LocalSet，Driver动态加入退出
    单个Driver关闭只结束该任务，不关闭工作区线程
    工作区线程JoinHandle由Workspace的关闭能力持有，Agent和LuaRuntime不各存一份
    Agent state保存操作关联，close数据只保存执行取消能力，ended独立报告完成，二者不是重复状态机
```

# error

## 类型

公开，共享错误契约：
```text
LuaRuntimeError：运行时错误，公开结构体，定义于共享lua_runtime_components.rs
    kind: LuaRuntimeErrorKind--类别
    description: String--UTF-8边界安全截断至最多512字节的详情
    new(kind: LuaRuntimeErrorKind, description: impl Into<String>) -> Self
        构造有界错误：公开方法
    impl Clone + Debug + PartialEq + Eq for LuaRuntimeError
    impl fmt::Display for LuaRuntimeError
    impl std::error::Error for LuaRuntimeError

LuaRuntimeErrorKind：运行时错误类别，公开枚举
    InvalidRequest
    InstanceNotFound
    InstanceMismatch
    AlreadyReady
    NotReady
    SourceTooLarge
    EnvironmentFailed
    CompileFailed
    ExecutionFailed
    StartupTimeout
    Cancelled
    CleanupFailed
    TaskFailed
    StopAlreadyWaiting
    EndNotificationLost
    AlreadyWaiting
    Closed
    WakeNotificationLost
```

## 逻辑

```text
安全边界：
    ready前一般失败必须在清理后回复；CleanupFailed意味着仍可能有活动，Agent不能按普通启动失败直接销毁
    Agent仅发StopDriverLuaVm，关闭动作由LuaRuntime取句柄完成
    设置取消、关闭wake和真实执行清理完成是三个不同动作，不以其中一个代替另一个；ended Ok还必须证明MCL队列回执收口
    活动Inference与Other不阻止沙箱切换但停止必须等待；agent_cleanup_tasks不取消Inference或Other，不得与根停止混为同一动作
    未配合取消的原生阻塞或宿主执行不能被描述为已经安全停止
    ready后异常由独立异常链通知，不替代已确定的启动结果或在途停止结果
```
