# 伪代码格式

```text
模块：使用一级标题，只写当前设计涉及的部分
类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明
函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束
    行为：多步逻辑只使用一个行为块，在其下缩进展开
逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
字段说明使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# 板块格式

```text
lib        插件与共享AgentExecutionScope契约
system     src/system.rs             单条工具请求调度
handler    src/handler/*.rs          快照准备与runner监督
events     共享types/tool_events.rs   请求、oneshot输出和内部关联
types      src/types.rs及runner协议   执行快照、限额和进程协议
error      共享工具错误契约          执行、取消与清理错误
```

**CallableResource snapshot请求及MCL typed arrays已实现；普通tool runner提供取消与真实cleanup；可信supervisor与受限worker分离，兼容既有工具Lua环境，长期任务协议仍为未来约束。** MCL选择不可变可调用资源快照并管理调用批次，ToolPlugin每个oneshot请求只执行一个匹配调用，独立runner基本协议不变。

当前MCL只有MESSAGE、TOOL、SANDBOX三种typed arrays；TOOL是资源值，保存ResourceID、alias、description、schema与immutable shared snapshot，不是ToolCall请求数组。`IMPORT TOOL tool AS alias`与`GET alias`解析Agent ImportBindings，不新增插件注册表；`infer`接收MESSAGE array + TOOL array，不接受ResourceID array或重复global alias query。旧Block不会跟随reload暗变，新GET alias获取新快照。CallableDefinitionSnapshot 只有 Tool，其他资源类型不能导入 TOOL。

## 用户轮次中止契约（已实现）

工具受理及独立turn_cancel监督已实现；活动登记原子保存turn与沙箱快照。无 `get_turn`；状态采用 `creating/idle/working/stopping`。中止立即清空 turn，清理期间仍为 Working，正常 finish 收口才 Idle；同步必须同时包含 turn 与 status。用户轮次中止与根 Agent shutdown、owner abort、timeout、receiver closed 不同，不统一归一为正常取消。

用户轮次取消且真实cleanup成功，oneshot返回Ok ToolExecuteResponse，其ToolCall消息failed=true、content="Tool call aborted."，保留调用ID、名称和已知resource_id。未启动调用同用此规范正文；不伪造执行成功，不撤销已经发生的副作用。CleanupFailed仍返回Err并保留活动，不能伪造取消或空闲。

AbortSystem先于MCL实际执行与工具受理；每个消费者独立观察共享中止事件，不靠take reply独占通知。实际受理再次核对当前turn与请求turn；None或已中止旧轮次不启动runner，返回正常取消结果。MCL批次保留已完成真实结果，其余调用（包括未启动项）按原顺序每项恰有一个取消结果。

## 当前执行机制与保留边界

以下runner机制与snapshot请求均已实现；资源选择准备消费已选定metadata snapshot，其余进程、取消及真实cleanup保证保留。

- 主线程认领共享 reply Sender并复制Agent/turn，消费请求选定的CallableResource定义与执行材料而非按ResourceID重取ResourceSet最新版本；使用共享作用域原子登记和沙箱快照。排队任务丢弃会撤销未启动登记并回复错误。
- 通用异步闭包启动独立 supervisor；调用者 Future 被取消或丢弃时请求取消，supervisor 仍持有执行所有权，完成真实进程、管道及临时目录清理后才 mark。执行所有者意外丢弃或清理证明缺失时保留登记并 `mark_cleanup_failed`。
- `src/process.rs` 并行投递 stdin、排读有界 stdout/stderr、等待进程，统一截止时间、作用域取消、MCL logical 取消及 receiver 关闭监督。stdout 仅 JSON 协议，Lua print 写 stderr；非法协议或缺少清理证明不视为执行已回收。
- Linux 工具进程分为可信 supervisor 与受限 worker：supervisor 位于工具沙箱之外，启用 child subreaper 并负责读取进程元数据、终止及 waitpid 回收所有后代，包括新 session；只有执行 Lua 的 worker 及其子进程进入本次 landstrip 策略。策略可以继续禁止工具读取 /proc，清理监督不能放进同一个禁读边界。父进程以独立 childgroup 为强杀兜底；无法证明后代已收口时明确 CleanupFailed，不假称进程组能保证清理。
- 可选 cgroup v2 执行边界通过 `MARGATROID_TOOL_CGROUP_ROOT` 指向已委派、可写且有 `cgroup.kill` 能力的真实 cgroup 根目录；runner 进入专用子组后才执行 Lua，清理要求 kill、populated 0 及目录移除。未配置时不声称具备 cgroup 隔离；配置不可用失败关闭。
- 父进程启动可信 tool_runner supervisor，supervisor 通过现有 landstrip CLI `run -p policy -- tool_runner --margatroid-tool-worker` 启动受限 worker；策略激活而后端不存在不会退化为无沙箱。环境过滤凭据及宿主注入变量，工具不获得修改 supervisor 策略与执行模式的宿主入口。
- shell 默认使用并行有界 stdout/stderr 管道。Linux 显式 `shell(command, {pty=true})` 使用非阻塞 AsyncFd PTY，建立 session 与控制终端，stdin/stdout/stderr 都连接 slave；终端输出合并在 `stdout`，`stderr` 为空，退出状态同时返回 `exit_code` 与兼容 `code`，并返回 `truncated`；max_output_bytes及timeout_ms按宿主总限额截断，不允许调用参数放宽总预算。PTY 读取和等待并行，取消、限额、超时及后代回收沿用同一执行预算；非 Linux 明确拒绝 PTY。
- handoff 第一版固定 UnsupportedHandoff，未增加任务池、全局 pending 或旧资源注册入口。

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。

# lib

## 类型

公开：
```text
ToolPlugin：工具执行插件，公开结构体
    schedule: String--准备System计划
    runner_path: PathBuf--独立runner程序路径
    limits: ToolExecutionLimits--执行、进程和输出限额
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法
        行为：保存计划名并返回自身。
    impl Plugin for ToolPlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                检查计划、通用闭包执行及共享资源设施，拒绝重复安装
                保存执行配置并挂载tool_execute_system
                不加载资源metadata、不安装资源注册System、不创建工具Entity或长期runner池

AgentExecutionScope：Agent所属共享执行作用域，公开共享契约--保存在Agent.execution，不另挂平行活动组件
    说明：统一记录Inference、Sandbox、Other活动、取消与真实cleanup等待，普通runner登记为Sandbox活动；IMPORT等无沙箱操作登记Other，MCL不对同一实际操作重复登记
        实际执行体由执行设施拥有，不能以独立全局进程表替代Agent归属
        定义及受检操作归共享执行领域components，公开签名仅引用共享公开类型，不依赖插件私有Handler
        活动登记、策略快照取得和沙箱更新使用同一互斥门；提交失败撤销登记，真实cleanup后mark_completed才释放
        长期任务完整协议延后，第一版普通runner可实现，但拒绝handoff请求为UnsupportedHandoff
```

## 逻辑

```text
职责：
    MCL完成TOOL可见快照与alias匹配，ToolPlugin请求直接带选定CallableResource，自带ResourceID；不按名称另选资源或按ResourceID重取latest
    ToolPlugin不管理批次、排序、turn生成、自动历史、推理或Driver邮箱；每个匹配调用仍为一个独立oneshot请求
    ToolResource加载由工具资源插件完成，本插件只消费选定snapshot并validate_owner校验已导入及许可
    tool统一执行独立runner中的Lua

进程隔离：
    独立runner是短生命周期工具进程，方便CLI沙箱工具包装，不在Workspace Driver线程创建工具VM
    独立进程不等于安全沙箱；无策略时明确为无沙箱，有策略但后端不可用必须失败
    不公开绕过runner在主进程执行任意工具Lua的普通调用入口
```

# system

## 函数

crate公开：
```text
tool_execute_system(world: &mut World)
    工具执行System：crate公开System
    处理事件：ToolExecuteRequest
    产出事件：ClosureRequest
    行为：克隆本帧请求并逐个调用handle_tool_execute，克隆只共享同一Sender槽；不轮询任务，不设置业务结果转发System。
```

## 逻辑

```text
异步任务直接通过oneshot返回，结果不需修改World
执行活动状态必须通过共享活动管理能力安全更新，不从执行线程借用World或修改ECS组件
通用闭包Result只用于监督，不能成为第二条权威工具结果通道
```

# handler

## 函数

crate公开：
```text
handle_tool_execute(world: &mut World, request: ToolExecuteRequest)
    准备单次执行：crate公开函数
    行为：
        认领共享Sender后检查Working及非空匹配turn，再检查轮次中止与受理关联，再校验请求ID、调用ID、完整Agent Entity、取消状态和资源类型；validate_owner(request.resource.resource_id)校验目标ResourceSet存在、driver_dependencies许可及ImportBindings的ID/TOOL kind已导入，不要求最新alias或定义与选定snapshot一致，允许仍有效的旧版本snapshot
        消费request.resource的不可变共享definition及alias，验证call.tool_name与选定alias一致；不从ResourceSet按ID重取latest，不再次global alias query
        tool使用选定schema与脚本材料引用及alias；不重取最新collection。现有路径late read不能证明磁盘内容版本隔离，后续采用Arc脚本正文或版本化文件材料的具体方案尚未闭合，不能把metadata snapshot不变假称为磁盘内容不变
        解码arguments并限制大小；tool按snapshot schema验证，不重新解析metadata或全局资源目录
        tool执行经Agent.execution共享作用域受检入口检查受理门、登记Sandbox活动并取得Agent.sandbox选定SandboxValue的policy及项目路径快照；SandboxValue自带resource_id，不按沙箱ID重取latest；取消令牌由该作用域派生
        活动登记与沙箱快照取得必须和沙箱切换互斥，不能先检查后异步登记形成竞态
        准备拥有所有权的数据，文件读取、临时目录创建及进程启动放入闭包，不阻塞ECS
        提交失败撤销未启动活动并完成错误回复；成功移交本次活动所有权和回复保障给闭包
```

私有：
```text
execute_tool(prepared: PreparedToolExecution)
    执行并完成回执：私有异步函数
    行为：
        tool有界读取snapshot选定的Lua脚本材料引用，建立本次临时目录并调用run_runner；执行中不重新解析alias或读取最新资源定义；可变磁盘路径的内容版本隔离仍为待验收项
        捕获普通错误与panic，清理本次拥有且未转交的执行资源
        真实清理完成后mark_completed再释放活动登记；清理失败保留可管理的活动关联，不假称已经空闲
        在共享reply短锁内take唯一Sender，发送一次成功或错误；目标用户轮次取消且cleanup成功返回Ok failed=true ToolCall，正文="Tool call aborted."；其他监督失败保持真实错误
        不记录message历史，由Driver显式history_append；不直接结束turn或发送Agent异常代替回执

run_runner(...) -> Result<Option<String>, ToolError>--Some为真实输出，None仅代表用户turn取消且cleanup已证明
    监督独立runner：私有异步函数
    行为：
        按本次沙箱策略建立执行边界，以结构化协议启动独立runner
        载荷带脚本文本、已解析arguments、上下文及限额，不让runner再次读取metadata或查询World
        stdin投递、stdout/stderr排读及进程等待并行协调，全部受总截止时间与取消监督
        stdout只承载协议结果，日志及Lua print重定向stderr；所有缓冲有界，不先wait再读取管道
        解析有界字符串结果，非零退出、无效协议、输出超限转换为明确错误
        Receiver关闭、超时或取消时终止本次未转交执行活动，等待退出并有界结束管道读取
        返回前处理残留子进程与临时目录；清理失败返回CleanupFailed，不静默删除活动记录

run_lua_in_runner(payload: RunnerRequest) -> Result<String, ToolError>
    执行runner内Lua：私有异步函数
    行为：
        在runner内创建工具VM，安装内存、指令及时间限制，按最小必要能力开放库与宿主
        在脚本顶层执行前准备arguments和context，加载脚本并取得execute入口
        execute(arguments, context)返回有界String；普通tool兼容既有结果契约：当字符串解析为含ok布尔与content字符串的JSON对象时，模型正文取content、failed取!ok；其他字符串原样成功
        context包含agent_id、resource_id、call_id、turn快照、project_root和temp_dir
        提供margatroid().json/shell及兼容全局json/shell、package.config和工具所需IO/OS文件能力；os.execute经有界异步shell返回标准Lua状态元组，不提供io.popen绕过进程预算。日志和普通stdout写入转诊断流，不污染JSON协议。不注入MCL、ready、wake或Driver调用能力，避免等待同Agent队列产生循环依赖
        shell单次超时和输出限额不能超过本次总预算，不接受参数放宽总限额
        普通shell使用管道，PTY作为显式选项；非零退出码由脚本决定是否转成工具失败

handoff_long_task(activity: ExecutionActivity, specification: LongTaskSpecification) -> Result<TaskID, ToolError>
    转交长期任务：私有未来接口占位，第一版一律返回UnsupportedHandoff
    行为：
        第一版不启动或移交长期任务；拒绝后本次调用仍拥有执行体并负责清理
        未来只有任务管理设施受理并接管真实执行体、输出、取消和清理所有权后才算转交成功
        成功返回task ID，runner可将ID作为结果返回并退出；本次清理不终止已接管任务
        转交失败仍由本次调用拥有执行体，必须清理；不能把后台进程脱离runner当作成功转交
        具体跨进程协议、任务创建/查询/输出接口后续设计，不假称转移任意子进程已被实现
```

## 逻辑

```text
MCL tool_call effect（当前输入契约）：
    参数顺序为MESSAGE array（恰好一条assistant）+ TOOL visible array，calls仅从assistant.tool_calls字段抽取
    TOOL只含可调用资源；ToolCall仍是assistant字段中的纯值，不升级为Block或独立typed array
    多条消息、非assistant、visible alias冲突或重复call ID必须在任何submit前使整个effect失败，不能部分提交后才报这些结构错误
    空calls返回emptyMessage，不执行工具；assistant不由infer隐式记录，tool_call effect也不重复记录
    对每个call按选定TOOL snapshot.alias匹配；每个匹配调用发送一个ToolExecuteRequest及oneshot，保持单调用基本runner不变
    匹配失败由MCL按原槽位构造关联错误；成功与失败结果均保留call ID，已选定资源保留resource_id，不虚构缺失资源ID
    MCL按原调用顺序收口tool结果，历史仅由显式history_append保存；所有已有message resource IDs必须保留，不因effect或history append丢失

正常调用：
    MCL选定CallableResource snapshot -> ToolExecuteRequest -> 主线程归属校验与活动登记
    独立runner读取选定材料 -> 清理本次执行资源 -> oneshot -> MCL填回原调用槽位；历史另发显式history_append
    System为完整消息变体，prompt按真实role导入且infer支持System；history append是独立允许范围，不含System，不改变infer输入范围

长期任务（未来约束，第一版拒绝handoff）：
    runner显式申请 -> 管理设施接管任务 -> 返回task ID -> runner退出
    长期任务保存完整Agent归属及启动时沙箱快照，之后不跟随Agent配置动态变化
    已结束且执行资源已清理的任务可以保留结果供LLM查询，不因结果未读取阻止切换沙箱
    状态变化先保存真实状态和输出，再按约定通知共享wake槽位；无等待者锁存，唤醒可合并
    不通过向MCL FIFO追加wake命令来完成通知，Driver醒来后查询真实任务状态

沙箱切换与重载：
    普通runner、运行中的长期任务以及取消中或清理失败的Sandbox活动均阻止改变沙箱配置；Inference与Other活动不阻止沙箱配置改变
    检查与配置修改在同一受检入口，活动启动也走同一受理机制，不依赖MCL FIFO消除所有竞态
    重复选择完全相同的当前配置可幂等成功；同ID但策略内容变化属于配置改变
    激活沙箱同ID重载也不能绕过此约束；新材料可先准备，发布改变前须检查活动，忙时保留原条目
    其他资源重载不改变已有typed Block或已开始任务的不可变共享快照；新GET alias取得新版本，工具执行始终使用选定snapshot而非resource_id重取latest；任务查询结果不视为仍拥有执行资源

清理接口：
    agent_cleanup_tasks只清理该作用域的Sandbox活动，关闭新的Sandbox执行受理门，取消普通runner及长期进程并异步等待真实执行体退出和资源释放；不取消Inference或其他非沙箱活动
    成功才mark_completed移除已清理Sandbox活动并恢复Sandbox执行受理；失败保留未完成活动管理状态且不重新放开Sandbox执行，继续禁止沙箱切换
    不直接clear任务表，不阻塞ECS，不删除已保存任务结果
    单独清理成功后恢复受理时，后续切换仍再次检查；不声称清理与切换天然原子；若整个作用域已经停止，不得由单独清理重新打开
    清理后立即切换的连续操作可将来设计，本次不增加第二个复杂入口

清理保证：
    进程组kill不能保证覆盖新session或脱离进程；沙箱/平台监督设施必须提供真实执行边界
    无法保证回收时报告CleanupFailed，不假称oneshot关闭代表执行已经停止
    Agent停止针对整个AgentExecutionScope，关闭所有活动受理、cancel全部活动并等待真实cleanup及MCL drain；Driver取消信号或oneshot关闭本身不等于活动退出
```

# events

## 类型

公开，全部载荷定义于共享types/tool_events.rs，不保留插件events.rs：
```text
ToolExecuteRequest：单条工具执行请求，公开事件
    id: String--本次执行请求唯一关联
    agent: Entity--完整目标Agent实例
    resource: CallableResource--MCL选定的不可变共享资源snapshot，自带resource_id、alias、definition；不另设重复resource_id/snapshot字段
    call: ToolCall--调用ID、tool_name和arguments等既有内容
    turn: Option<String>--提交时轮次快照；目标在实际受理再次校验，None或已中止旧轮次不启动执行
    cancel: CancellationToken--所属执行作用域派生取消令牌
    reply: Arc<Mutex<Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>>>
    impl Event for ToolExecuteRequest

ToolExecuteResponse：单条成功响应，公开共享结构体--oneshot载荷，不是结果事件
    message: AgentMessage--ToolCall消息，resource_id: Option<ResourceID>为Some(已选定ID)，保留调用ID及正文；未找到资源的错误由MCL填None，不虚构地址
```

## 逻辑

```text
成功或失败只通过原oneshot完成，批次失败模板由MCL生成；目标正常用户轮次中止是Ok failed=true消息，不是执行成功证明
准备失败也必须完成reply，独立runner协议错误不允许丢失调用关联
任务创建、查询、清理的内部异步载荷未来统一定义于任务领域events，不暴露插件私有状态
```

# types

## 类型

公开共享纯值契约：
```text
CallableResource：TOOL元素，公开共享纯值契约--归共享types，本插件不另建注册表
    resource_id: ResourceID--选定导入资源身份，validate_owner只校验已导入及许可，不取新版本
    alias: String--模型可见及call匹配名称
    definition: Arc<CallableDefinitionSnapshot>--不可变共享description、input_schema与资源种类/执行材料引用
    说明：已有Block和请求持有同一snapshot；Agent ImportBindings是alias权威，GET alias取得当前绑定快照。请求仅resource持有此值，不重复ID和snapshot字段。

ToolExecutionLimits：执行限额，公开结构体
    script_bytes: usize
    argument_bytes: usize
    output_bytes: usize
    diagnostic_bytes: usize
    memory_bytes: usize
    instruction_budget: u64
    execution_timeout: Duration
    host_call_timeout: Duration
    cleanup_timeout: Duration
    说明：所有上限非零，宿主子预算不得扩大执行总预算；具体默认值实施前确定。
```

私有：
```text
PreparedToolExecution：执行快照，私有结构体--闭包捕获，不是事件
    request: ToolExecuteRequest
    说明：资源身份、alias、schema和材料引用统一由request.resource: CallableResource拥有，不再另设resource: ToolExecutionResource或复制独立resource_id字段；runner只接收该snapshot派生的必要协议值。
    arguments: serde_json::Value--已验证参数
    project_root: PathBuf
    sandbox_policy: Option<SandboxPolicySnapshot>--本次执行固定策略
    activity: ExecutionActivity--tool持有Sandbox活动，真实cleanup后释放
    runner_path: PathBuf
    limits: ToolExecutionLimits

RunnerRequest：runner输入，私有结构体
    protocol_version: u32
    script: String
    arguments: serde_json::Value
    context: ToolContext
    limits: ToolExecutionLimits

RunnerResponse：runner输出，私有结构体
    protocol_version: u32
    result: Result<String, RunnerErrorPayload>

任务领域占位：
    AgentExecutionScope、ExecutionActivity、SandboxPolicySnapshot由共享执行领域提供，不依赖工具插件私有类型
    TaskID和LongTaskSpecification属于未来共享任务协议，第一版不要求实现handoff
    此处只描述归属及能力需求，不新增全局TaskRegistry、runner池或第二套Agent生命周期
```

# error

## 类型

公开，共享工具错误契约：
```text
ToolErrorKind：工具错误分类，公开枚举
    InvalidRequest
    AgentNotAlive
    ResourceMissing
    UnsupportedResource
    UnsupportedHandoff
    InvalidArguments
    InvalidDefinition
    ScriptReadFailed
    ExecutionBusy
    SandboxUnavailable
    SubmitFailed
    RunnerStartFailed
    RunnerProtocolFailed
    ExecutionFailed
    OutputTooLarge
    Timeout
    Cancelled
    TaskPanicked
    CleanupFailed

ToolError：工具错误，公开结构体
    kind: ToolErrorKind
    message: String--有界且不泄漏环境凭据
    impl Clone + Debug + PartialEq + Eq for ToolError
    impl fmt::Display for ToolError
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：公开trait方法
            行为：输出分类及有界描述。
    impl std::error::Error for ToolError
```

## 逻辑

```text
单条工具失败不直接结束turn，不通过AgentException替代原请求结果
清理失败须保留真实活动关联，禁止将错误回复等同于进程清理成功
普通runner执行、作用域登记、取消和真实cleanup等待保持既定要求；typed请求已实现，第一版仍不支持handoff

当前同步验收与限制（未闭合项不得标为已完成）：
    验证MESSAGE/TOOL/SANDBOX三个typed arrays、IMPORT TOOL tool AS alias和GET alias，资源值与ToolCall纯值严格区分
    验证infer使用MESSAGE + TOOL及已准备definition/validate_owner，不接受ResourceID array、不重复global alias query
    验证tool_call所有结构校验在submit前完成、空calls返回emptyMessage、不隐式记录输入assistant或结果、每匹配调用单oneshot及显式历史消息保留IDs
    验证System/prompt真实role、infer接收System，与history append不含System的独立范围
    验证reload后旧Block/已选定snapshot不变，新GET获得新快照；ToolExecuteRequest.resource是唯一CallableResource持有字段，Agent.sandbox使用选定SandboxValue策略
    脚本材料的磁盘版本隔离方案尚未闭合；现有路径late read不提供该保证，需要进一步选择Arc正文或版本化材料并验收
    平台沙箱边界及长期任务完整协议继续单独设计；保留取消后join进程、排读管道、后代回收、临时目录清理的真实证明要求
```
