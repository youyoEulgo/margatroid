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
handler    src/handler/*.rs          快照准备、runner监督与skill读取
events     共享types/tool_events.rs   请求、oneshot输出和内部关联
types      src/types.rs及runner协议   执行快照、限额和进程协议
error      共享工具错误契约          执行、取消与清理错误
```

**本文是尚未实现的目标设计。当前代码参考仍保存在DESIGN.md。** MCL选择资源并管理调用批次，ToolPlugin执行一条明确ResourceID的调用，独立runner执行Lua，oneshot返回结果。长期任务是未来能力，本文件先确定所有权、沙箱和清理约束，不假称已有任务管理设施。

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
    说明：统一记录Inference、Sandbox、Other活动、取消与真实cleanup等待，普通runner登记为Sandbox活动；skill/IMPORT等无沙箱操作登记Other，MCL不对同一实际操作重复登记
        实际执行体由执行设施拥有，不能以独立全局进程表替代Agent归属
        定义及受检操作归共享执行领域components，公开签名仅引用共享公开类型，不依赖插件私有Handler
        活动登记、策略快照取得和沙箱更新使用同一互斥门；提交失败撤销登记，真实cleanup后mark_completed才释放
        长期任务完整协议延后，第一版普通runner可实现，但拒绝handoff请求为UnsupportedHandoff
```

## 逻辑

```text
职责：
    MCL完成可见ResourceID筛选及alias匹配，ToolPlugin请求直接带选定ResourceID，不再按名称选择另一个资源
    ToolPlugin不管理批次、排序、turn生成、自动历史、推理或Driver邮箱
    ToolResource、SkillResource加载由对应资源插件完成，本插件只消费目标Agent集合中的条目
    tool执行独立runner中的Lua；skill有界读取正文并返回tool_call消息，不启动Lua
    hook由Driver解释，不在ToolPlugin执行；prompt和sandbox不是工具调用入口

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
        校验请求ID、调用ID、完整Agent Entity、取消状态和资源类型
        从该Agent的ResourceSet按resource读取条目，tool复制已加载schema与脚本路径，skill复制正文路径与alias快照，不读取不存在的skill schema
        解码arguments并限制大小；tool按已加载schema验证，skill使用消费侧固定schema {"type":"object","properties":{},"additionalProperties":false}，只接受空对象{}，与Inference展示定义完全一致；不新增skill schema文件，不重新解析metadata或全局资源目录
        tool执行经Agent.execution共享作用域受检入口检查受理门、登记Sandbox活动并取得当前沙箱策略与项目路径快照；取消令牌由该作用域派生
        活动登记与沙箱快照取得必须和沙箱切换互斥，不能先检查后异步登记形成竞态
        skill由ToolPlugin在同一作用域登记Other活动并监督正文读取及输出限额；它不阻止沙箱切换，但Agent停止仍取消并等待读取清理与调用收口
        准备拥有所有权的数据，文件读取、临时目录创建及进程启动放入闭包，不阻塞ECS
        提交失败撤销未启动活动并完成错误回复；成功移交本次活动所有权和回复保障给闭包
```

私有：
```text
execute_tool(prepared: PreparedToolExecution)
    执行并完成回执：私有异步函数
    行为：
        skill有界读取已验证资源路径的UTF-8正文，将内容构造为tool_call消息
        tool有界读取Lua脚本，建立本次临时目录并调用run_runner
        捕获普通错误与panic，清理本次拥有且未转交的执行资源
        真实清理完成后mark_completed再释放活动登记；清理失败保留可管理的活动关联，不假称已经空闲
        在共享reply短锁内take唯一Sender，发送一次成功或错误；取消和任务丢弃也必须关闭或完成回复
        不记录历史，MCL逐项记录成功和失败；不直接结束turn或发送Agent异常代替回执

run_runner(prepared: &PreparedToolExecution) -> Result<String, ToolError>
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
        execute(arguments, context)返回有界String作为工具正文，不隐式解包字符串内JSON信封
        context包含agent_id、resource_id、call_id、turn快照、project_root和temp_dir
        提供JSON编解码和shell；不注入MCL、ready、wake或Driver调用能力，避免等待同Agent队列产生循环依赖
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
正常调用：
    MCL选定资源 -> ToolExecuteRequest -> 主线程快照与活动登记
    独立runner或skill读取 -> 清理本次执行资源 -> oneshot -> MCL填回原调用槽位并记录历史

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
    其他资源重载不改变已开始任务的快照；任务查询结果不视为仍拥有执行资源

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
    resource: ResourceID--MCL已选择的资源身份
    call: ToolCall--调用ID、tool_name和arguments等既有内容
    turn: Option<String>--提交时轮次快照
    cancel: CancellationToken--所属执行作用域派生取消令牌
    reply: Arc<Mutex<Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>>>
    impl Event for ToolExecuteRequest

ToolExecuteResponse：单条成功响应，公开共享结构体--oneshot载荷，不是结果事件
    message: AgentMessage--ToolCall消息，resource_id: Option<ResourceID>为Some(已选定ID)，保留调用ID及正文；未找到资源的错误由MCL填None，不虚构地址
```

## 逻辑

```text
成功或失败只通过原oneshot完成，批次失败模板由MCL生成
准备失败也必须完成reply，独立runner协议错误不允许丢失调用关联
任务创建、查询、清理的内部异步载荷未来统一定义于任务领域events，不暴露插件私有状态
```

# types

## 类型

公开共享纯值契约：
```text
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
    resource: ToolExecutionResource--tool脚本/schema或skill正文路径快照
    arguments: serde_json::Value--已验证参数
    project_root: PathBuf
    sandbox_policy: Option<SandboxPolicySnapshot>--本次执行固定策略
    activity: ExecutionActivity--tool持有Sandbox活动，skill持有Other活动；均真实cleanup后释放
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
普通runner执行、作用域登记、取消和真实cleanup等待为第一版既定要求；平台沙箱边界和长期任务完整协议仍需详细设计，第一版不支持handoff
```
