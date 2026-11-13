# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System分别列出事件、闭包结果、提交闭包和oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
方法使用下列层级；--仅承载简短说明，执行逻辑写入行为。
method(args) -> xxx
    行为：
        xxx
```

# 板块定义

```text
# lib       tool/core/src/lib.rs；tool/plugin/src/lib.rs  结果校验入口；Plugin安装
# components tool/core/src/components.rs                ToolTasks
# events    tool/core/src/events.rs                      InitToolTasks、ToolExecuteRequest
# system    tool/plugin/src/system.rs                    工具请求路由
# handler   tool/plugin/src/handler.rs                   同步受理与独立runner监督
# types     tool/plugin/src/types.rs                     执行上下文及runner协议
# types pub tool/core/src/types.rs                       工具调用、结果及预算
# error     tool/core/src/error.rs                       ToolError
```

# lib

## 函数

公开，定义于tool_core：
```text
parse_tool_result(value: &serde_json::Value) -> Result<ToolExecuteResponse, ToolError>
    行为：
        字符串原样作为文本，其他值序列化为content；
        识别ok/content/images结果信封；
        校验结果信封、图片编码与格式，不限制数量和字节，不创建消息领域类型。
```

## 类型

公开，定义于tool_plugin：
```text
ToolPlugin：工具执行插件
    schedule: String
    runner_path: PathBuf
    limits: ToolExecutionLimits
    impl Plugin for ToolPlugin
        build(self, app: &mut App)
            行为：
                require 执行设施可用、预算有效、未安装
                require schedule in [RuntimePlugin::UPDATE, RuntimePlugin::POST_UPDATE]
                install init_tool_tasks_system, tool_execute_system
```

# components

## 类型

公开：
```text
ToolTasks：该Agent的工具任务句柄集合，公开组件
    shared: Arc<TaskSetShared>--唯一集合及版本，内部可变字段私有
    updates: TaskUpdateContext--必需，同一Agent的所有任务写能力从此衍生
    new(owner: Entity, address: String, binding: TaskSyncBinding) -> Self
        行为：先创建TaskSetShared，再由binding.bind传入Weak生成必需TaskUpdateContext；不允许缺少同步绑定的生产构造。
    update_context(&self) -> TaskUpdateContext
    close_sync(&self)
        行为：实际来源结束时失效source并移出初始化索引。
    register(&self, id: String, handle: ToolTaskHandle) -> Result<(), ToolError>
        行为：调用update_task执行本次登记，不直接写集合。
    cancel(&self, id: &str) -> Result<(), ToolError>
        行为：
            向对应句柄请求取消，不直接中断监督任务或删除句柄。
    list(&self) -> Vec<(String, ToolTaskHandle)>
        行为：
            return entries中的活任务句柄克隆
    snapshot(&self) -> Vec<ToolTaskSnapshot>
        行为：在TaskSetShared更新门内取得当前完整活跃集合，按task_id稳定排序；快照版本与该次读取一致。
            每项包含完整保留窗口；不调用read、不推进cursor、不清除replaced、不发送reply、不触发取消或finish。
            所有同步可见更新受同一Agent的更新门串行化；释放锁后才编码和投递，不持锁等待IO。
    read(&self, id: &str) -> Result<TaskReadResult, ToolError>
        行为：
            从集合取得仍存活任务后，同一短锁读取output[cursor..]并将相邻字符串无分隔符拼成一项，非字符串原样保留顺序；
            读取活任务error，将cursor推进至当前末尾，返回并清除replaced标记。
    send(&self, id: &str, value: serde_json::Value) -> Result<(), ToolError>
        行为：
            向目标Lua输入通道投递一个值；
            目标已finish返回TaskMissing；
            通道关闭或满返回错误，不宣称接收方已处理。
    finish(&self, id: &str) -> Future<Output = Result<(), ToolError>>
        行为：
            request cancel(id)
            await Lua、进程及后代、管道、临时资源真实清理
            success: 调用update_task执行真实CompleteCleanup移除并释放缓存，随后返回Ok(())
            failure: return error; retain supervision
    abort_unreturned(&self) -> 工具中止交接结果
        行为：
            for handle in entries:
                atomic if !handle.returned:
                    reply 原call_id/alias的中止结果
                    通过update_task执行原子AbortReturn原子更新returned
                    请求该句柄取消
                    同步触发，不等待清理
    impl Clone for ToolTasks
        行为：
            共享同一集合，不借用World。
    impl Component for ToolTasks
```

# events

## 类型

公开：
```text
InitToolTasks：工具任务组件初始化请求，公开事件
    agent: Entity
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), ToolError>>>>>
    impl Event for InitToolTasks

ToolExecuteRequest：单次工具请求，公开事件
    id: String
    agent: Entity
    turn: String--发出请求时的回合标识，System受理必须与Agent当前turn匹配
    alias: String--工具路径集合的键
    call_id: String--模型调用关联
    arguments: serde_json::Value
    reply: Arc<Mutex<Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>>>
    impl Event for ToolExecuteRequest
```

# system

## 函数

crate公开：
```text
init_tool_tasks_system(world: &mut World)
    处理事件：InitToolTasks
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    oneshot回复：Result<(), ToolError>
    行为：
        InitToolTasks -> handle_init_tool_tasks。

tool_execute_system(world: &mut World)
    处理事件：ToolExecuteRequest
    接收闭包结果：Result<ToolLaunchOutput, ToolLaunchError>
    提交闭包：launch_tool(ToolExecutionContext) -> Future<Output = Result<ToolLaunchOutput, ToolLaunchError>>
    产出事件：None
    oneshot回复：Result<ToolExecuteResponse, ToolError>
    行为：
        在中止schedule之后执行；
        ToolExecuteRequest -> handle_tool_execute，逐请求复检当前turn，不使用发出时状态代替受理检查。
```

# handler

## 函数

crate公开：
```text
handle_init_tool_tasks(world: &mut World, request: InitToolTasks)
    行为：
        认领reply，检查agent实体；
        不存在ToolTasks则挂载空集合，已存在不覆盖；
        组件可查询后回复Ok(())，失败回复ToolError。

handle_tool_execute(world: &mut World, request: ToolExecuteRequest)
    行为：
        认领reply，核对Agent完整Entity、Working及当前非空turn与request.turn匹配；
        无turn或不匹配直接返回保留call_id/alias的中止工具结果，不注册任务、不启动闭包或进程。
        entry = Tools.get(request.alias) or return 对应工具错误
        使用entry.input_schema校验arguments
        按entry.path执行；
        owner_task_id与binding放入context
        从SelectedSandbox及Sandboxes取得可选策略路径
        模型参数不能覆盖宿主context标记
        固定本次工具、策略路径和目录参数，准备owned执行材料，不跨await借用World。
        受理后ToolTask独立持有执行材料；之后Tools记录被删除不影响该任务。
        从ToolTasks.updates衍生同一来源的ToolTaskHandle，经统一Register入口登记，再把该能力克隆交给工作任务；
        通过send_async_closure提交launch_tool，失败通过原reply回复错误；
        不缓存执行器Handle。
```

私有：
```text
update_task<R>(context: &TaskUpdateContext, change: impl FnOnce() -> Result<(R, bool), ToolError>) -> Result<R, ToolError>
    行为：取得必需context.gate，在同一更新门执行具体修改，失败不发布；返回的changed仅在同步可见状态实际改变时为true。同门内递增revision，释放门后publish到共享Client槽位。
    生产唯一写能力由ToolTaskHandle/ToolTasks方法构造具体闭包，内部可变字段保持私有；网络准备只持Weak集合，读取完整快照时取得同一更新门。

launch_tool(context: ToolExecutionContext) -> Future<Output = Result<ToolLaunchOutput, ToolLaunchError>>
    行为：
        在普通异步闭包内部创建本领域监督任务，立即把真实JoinHandle交给已登记的ToolTaskHandle，再返回启动结果。该闭包结果只证明启动交接，不表示工具完成；
        supervise_tool才负责业务reply和真实清理。启动异常记录于原ToolTask并完成原reply，System只收口启动事实，不反向控制其他领域。

supervise_tool(context: ToolExecutionContext) -> Future<Output = ()>
    行为：
        blocking worker按ToolEntry.path读取要执行的程序，schema直接使用Tools记录；
        sandbox_resource_core::read_sandbox读取策略，不重新读取工具定义。
        校验arguments与本入口的有效schema、具体后端策略；
        检查本任务取消状态后启动run_runner，动态入口不读取注册schema作回退。配置及execute收到相同可信owner_task_id/binding/resource_dir，不接受arguments覆盖。
        接收runner的Return时通过共享句柄的原子return裁决交付结果，不结束runner；
        中止、错误与正常Return共用唯一回复端，不能在context另持独立可回复状态；
        缓存写入、覆盖、正常/强制Return、错误及清理移除均调用update_task；任务输入与脚本read只保留各自原语义。
        自然退出/finish/到期/中止:
            World闭包: Tools.remove_owner(task_id)
            删除所属工具记录不检查调用状态，不等待或取消其他ToolTask
            未return则交付对应失败结果
            await VM、runner、后代、管道、临时文件真实清理
            调用统一CompleteCleanup入口移除本任务并释放缓存，直接通知完整task.sync集合变化
        已回复调用不得重复回复；
        后台异常记录于对应任务error，未回复调用按实际错误完成。
        清理失败及时交付CleanupFailed；
        保持未finish并保留本次runner管理能力继续清理，不标记已finish。

run_runner(context: &mut ToolExecutionContext, request: RunnerRequest, policy: Option<Value>) -> Future<Output = Result<(), ToolError>>
    行为：
        直接启动可信supervisor，传递执行材料及可选沙箱后端与策略。
        不建立内存cgroup、systemd scope或全局MemoryPool，不设置外部进程内存额度。
        并行投递stdin、读取stdout/stderr和等待退出，处理双向协议帧。
        取消或超时向supervisor发送终止请求，继续等待真实后代回收证明、supervisor退出和两端管道收口。
        清理失败保持监督，不以丢弃Future或收到Return冒充清理成功。
        Return不是进程终态，真实退出清理后返回。

supervisor_main(args: Vec<OsString>) -> Future<Output = i32>
    行为：
        Linux入口安装child subreaper，解析可选sandbox参数，校验私有请求协议和Lua限额。
        直接构造worker或sandbox run -p policy -- worker命令，不调用内存入组入口。
        传递Configured与原host/worker协议，等待终结结果、取消或退出信号。
        扫描、终止并回收全部后代，包括setsid和双重fork后代，等待worker及输出管道结束。
        只有真实清理完成才发送Stopped(cleanup_proven=true)，无内核OOM内存组状态映射。
        非Linux入口保持既有不可用行为，本次不新增跨平台进程回收实现。

run_tool_worker(request: RunnerRequest) -> Result<(), ToolError>
    行为：
        在受沙箱限制的worker内创建Lua，先以纯计算环境加载脚本并运行configure(arguments, context)，Lua VM内存上限始终生效，执行hook只处理取消及工具声明的deadline；
        创建只读元数据对象margatroid，context与arguments字段对应本次调用；configure阶段仅提供元数据访问。
        Lua全局只注入margatroid对象，不安装同名全局宿主函数、json表、args或context，也不保留margatroid()函数入口。
        公共实现不依赖Tool、MCL或Storage领域；本plugin通过其core契约构造能力回调并填入共享定义
        字段getter返回独立数据，Lua修改取得的表不改变宿主调用标记。
        异步方法同步取得内部共享句柄并释放userdata借用，再执行等待，支持同一对象并发调用。
        安装共享Margatroid对象定义，按tool身份、configure阶段和当前执行边界绑定允许的能力
        配置经Configured帧往返确认后启用公共执行方法与margatroid.tools与margatroid.task方法，再运行execute(arguments, context)，不重新加载脚本
        margatroid.driver方法始终拒绝，执行身份在宿主绑定时固定，不接受arguments或可写Lua表覆盖
        执行阶段os.execute和io.popen报错并提示使用margatroid.process:start。自然退出也进入清理；
        task_finish自身以控制退出终止后续Lua语句，不等待自身JoinHandle。
        在宿主入口边界捕获加载、configure与execute的Lua异常，先保留原始错误、程序来源、行号与traceback
        脚本加载前建立内部诊断发送能力，不向configure脚本开放日志或其他IO方法
        supervisor允许配置确认前的私有LuaDiagnostic帧，主动Log仍只允许execute阶段
        自动日志通过扩展的私有worker协议交给宿主，宿主补齐Agent、回合、任务和调用关联后写入agent_logs
        等待原始诊断写入处理完成，再交付简洁失败或释放本次执行存储能力
        原始诊断只写日志；返回给LLM的工具失败使用分类后的简洁说明，成功输出保持原样
        task_finish控制退出、主动取消和工具超时沿原控制流程处理，不冒充Lua脚本故障
        已task_return后发生异常仍记录诊断，但不得再次回复；后台任务error只保存可对外展示的说明
        自动诊断只由该入口捕获边界记录一次，不在上层重复记录同一异常
        诊断写入失败走现有stderr通道，保留原执行错误，不递归记录日志失败
```

## Lua宿主对象

```text
Margatroid：共享Rust userdata，Lua全局对象名margatroid
    context: serde_json::Value--只读身份、路径、调用关联及arguments，返回独立副本
    files: 文件操作子对象
    data: 数据格式转换子对象，含null空值标记
    text: 通用文本搜索子对象
    process: 子进程启动子对象
    driver: Driver协调子对象
    tools: 工具定义注册子对象
    task: ToolTask执行及交互子对象

margatroid:log(level: String, message: String, details: Option<LuaValue>) -> Future<Output = ()>
    行为：
        仅允许debug、info、warn、error；details必须为对象或省略
        经私有Log帧交宿主写入agent_logs，关联与来源由宿主固定
        等待SQLite写入确认后返回；失败明确报错，不递归日志、不追加会话或触发推理
        configure阶段不开放日志IO，阶段异常仍由入口自动记录

margatroid:wait_any(operations: Table<String, Function>) -> Future<Output = (String, LuaValue)>
    行为：
        并发等待命名操作，返回首个完成操作的名称与结果；操作失败则传播错误
        返回后释放剩余等待，空表报错；不取消对应Task或进程本身
        异步方法先克隆内部句柄并释放userdata借用，再await

margatroid.files:read(path: LuaString) -> Future<Output = LuaString>
    行为：
        相对路径基于当前执行环境的项目根，在既有沙箱和权限内读取原始字节
        返回完整内容，不加行号或结果包装；不存在、不可读或取消时返回实际错误

margatroid.files:write(path: LuaString, content: LuaString) -> Future<Output = ()>
    行为：
        写入指定路径的原始字节，不隐式创建父目录、不生成备份
        相对路径基于项目根；写入失败或取消如实返回错误，不伪造成功

margatroid.files:create_directory(path: LuaString, recursive: bool) -> Future<Output = ()>
    行为：
        recursive=false时只创建指定目录，父目录不存在则报错
        recursive=true时显式创建缺失的父目录；目标已是目录时成功，目标为文件时报错
        在当前执行环境权限内操作，不启动shell或外部mkdir进程

margatroid.files:info(path: LuaString) -> Future<Output = FileInfo>
    行为：
        合并文件属性与路径解析，path为本次查询项的绝对路径
        kind、size、modified_at_ns描述查询项自身，符号链接不冒充目标文件
        resolved_path为解析链接后的真实目标；失效链接目标不可解析时为None
        查询项不存在或属性不可访问时返回错误；路径和名称保留原始字节，不以替换字符损坏非UTF-8路径
        不把路径解析当作额外沙箱

margatroid.files:list(path: LuaString) -> Future<Output = Vec<DirectoryEntry>>
    行为：
        列出目录直接子项，按name排列，返回name及与info相同的文件信息
        不隐式忽略隐藏文件，不递归或跟随目录符号链接遍历
        工具负责递归、忽略规则、排序及结果格式；不可读或取消时返回错误

margatroid.files:matches(path: LuaString, pattern: String) -> bool
    行为：
        对路径字符串进行真正的glob匹配，不要求文件存在，不访问或遍历文件系统
        保留现有工具使用的分隔符、通配符、**、字符组及组合模式语义
        无斜杠模式的文件名匹配与含斜杠的相对路径匹配遵循现有ripgrep glob契约
        非法模式返回明确错误，不使用Lua pattern近似替代

margatroid.data:decode(format: String, content: LuaString) -> LuaValue
    行为：
        format仅接受json、yaml、toml、base64，不猜测格式，不保留旧方法别名
        JSON返回完整结构值，保留对象、数组、空容器、数字及null区别
        YAML解析单个文档，映射键须为字符串；重复键、不支持的标签及非有限数字报错
        TOML返回对象，日期时间转标准文本，不把任意字符串推断成日期
        Base64校验标准编码并返回原始字节；非法编码报错
        空值通过margatroid.data.null表示，不转为缺失字段

margatroid.data:encode(format: String, value: LuaValue) -> LuaString
    行为：
        format仅接受json、yaml、toml、base64，保留各标准支持的数据语义
        JSON与YAML编码可表达的结构化值；不支持的类型、循环表及非有限数报错
        TOML要求对象根值，null或不可表达的值报错，不删除字段或隐式字符串化
        Base64只接受字节字符串并输出标准编码

margatroid.text:search(pattern: String, content: LuaString) -> Vec<TextMatch>
    行为：
        用通用Rust正则引擎查找内容中的非重叠匹配，保留ripgrep默认正则及内联flags语义
        返回原始内容中的零基字节偏移半开区间[start,end)，不按Unicode字符数计偏移
        零宽匹配按引擎规则前进，确保搜索终止；非法表达式返回明确错误
        不遍历文件、不处理忽略规则、不生成grep输出，也不以Lua pattern代替正则
        行号、逐行搜索、二进制处理与结果截断由具体Lua工具决定

margatroid.process:start(program: String, arguments: Vec<String>) -> Future<Output = ProcessSession>
    行为：
        program必须为明确可执行路径，arguments为argv，不隐式拼接shell
        在当前执行环境的项目根及既有沙箱内启动原始stdin/stdout/stderr管道
        子进程归当前VM或ToolTask，随所属执行环境的取消、超时及真实清理回收
        只有Bash或终端工具显式启动shell-client，文件类工具不依赖该bin

margatroid.driver:mcl(target, instruction, arguments?) -> HostFuture
    行为：
        仅Driver身份可用，工具中调用明确返回身份错误
        沿用原MCL参数、身份绑定及结果契约，不建立工具任务

margatroid.driver:wait() -> HostFuture
    行为：
        仅Driver身份可用，等待原DriverWakeSlot；工具中调用明确失败

margatroid.tools:register(definitions: Vec<InjectedToolDefinition>) -> Future<Output = ()>
    行为：
        仅工具execute阶段可用，Driver中明确失败；操作Tools记录，不操作运行任务
        宿主固定程序path为当前资源，每项owner_task_id由调用方显式配置
        World闭包核对当前任务仍存在且未取消，再校验整批定义和alias冲突
        省略owner为静态工具；指定owner要求同Agent对应任务存活且未进入清理
        整批登记，任一错误则全批不写入；不覆盖已有记录
        指定owner结束仅移除其工具记录，不等待或取消已经受理的ToolTask

margatroid.task:append_output(value: LuaValue) -> Future<Output = ()>
    行为：
        仅工具execute阶段可用，向当前任务缓冲追加值
        窗口超限滚出旧项并调整cursor；超长字符串保留UTF-8尾部，放不下的结构项整项滚出

margatroid.task:replace_output(value: LuaValue) -> Future<Output = ()>
    行为：
        仅工具execute阶段可用，替换当前输出缓冲，cursor=0且replaced=true
        替换与读取共用短锁，不改变任务存活状态

margatroid.task:read_output(task_id: String) -> Future<Output = TaskReadResult>
    行为：
        仅工具execute阶段可用，读取同Agent任务增量缓冲并推进cursor
        不返回completed字段，已经finish的目标返回TaskMissing

margatroid.task:return_result(value: LuaValue) -> Future<Output = ()>
    行为：
        仅工具execute阶段可用，原子交付原调用一次结果，不结束VM或关闭输入
        与中止共用原reply裁决，重复回复报错，成功交付后异常仍只记录日志

margatroid.task:finish(task_id: String) -> Future<Output = ()>
    行为：
        仅工具execute阶段可用，只操作同Agent任务；Driver中调用明确失败
        自身以控制退出结束Lua，由监督者真实清理；不等待自己的JoinHandle
        其他任务在真实清理并移除后才返回成功，已finish目标返回TaskMissing

margatroid.task:send(task_id: String, value: LuaValue) -> Future<Output = ()>
    行为：
        仅工具execute阶段可用，向同Agent任务输入通道投递结构化值
        成功仅表示已入队，不表示目标已经处理

margatroid.task:receive() -> Future<Output = LuaValue>
    行为：
        仅工具execute阶段可用，等待当前任务的一条输入，不持缓存锁
        消息协议由Lua工具解释，等待服从当前任务取消及timeout

ProcessSession：原始子进程userdata
    write(bytes: LuaString) -> Future<Output = ()>
        行为：
            按写入顺序写入完整stdin字节，保持取消后同帧续写及背压语义
    read() -> Future<Output = {data: LuaString, eof: bool}>
        行为：
            读取stdout字节块及EOF，消息边界由工具解释
    read_stderr() -> Future<Output = {data: LuaString, eof: bool}>
        行为：
            独立读取stderr字节块及EOF
    wait() -> Future<Output = {exit_code: Option<i32>, signal: Option<i32>}>
        行为：
            等待子进程退出并回收，返回真实退出状态
    close() -> Future<Output = ()>
        行为：
            终止并回收进程及管道，不自动finish当前ToolTask
```

# types

## 类型

私有：
```text
ToolLaunchOutput：监督任务已交给ToolTaskHandle的启动事实，不是工具执行结果
ToolLaunchError：ToolError及原请求回复/共享句柄关联，实现From<AsyncTaskError>

ToolExecutionContext：单次执行材料
    request: ToolExecuteRequest
    reply：唯一回复端由ToolTaskShared持有，context仅通过同一共享句柄交付
    tool_entry: ToolEntry--按当前alias找到的工具记录，包含schema、path及必要任务标记
    sandbox_path: Option<PathBuf>
    project_root: PathBuf
    runner_path: PathBuf
    limits: ToolExecutionLimits
    handle: ToolTaskHandle--与集合条目指向同一任务
    tasks: ToolTasks--共享集合句柄，处理task_read/task_send/task_finish不借用World
    storage: AgentStorage--当前Agent的owned日志写入能力，不传入worker
    agent_name: String--宿主固定真实名称及调用关联
    diagnostic_received: bool--仅本次调用，用于避免同一worker故障重复记录

RunnerContext：进程执行材料--本次读取的程序、校验后的参数、策略及执行预算

RunnerRequest：stdin协议
    protocol_version: u32
    program: String
    arguments: serde_json::Value
    context: ToolContext
    limits: ToolExecutionLimits

RunnerFrame：worker到supervisor的有序协议帧，私有枚举
    Return(serde_json::Value)
    Write(serde_json::Value)
    Replace(serde_json::Value)
    Read { request_id: String, task_id: String }
    Send { request_id: String, task_id: String, value: serde_json::Value }
    Finish { request_id: String, task_id: String }
    Inject { request_id: String, definitions: Vec<InjectedToolDefinition> }--通用工具定义登记请求，不含MCP协议
    Log { request_id: String, record: AgentLogRecord }--主动日志，宿主固定可信关联并在写入完成后确认
    LuaDiagnostic { request_id: String, record: AgentLogRecord }--含原始错误、来源和traceback，只在宿主日志路径消费并确认
    InputConsumed--Lua取走输入后允许转发下一条；宿主响应读取不等待输入邮箱腾位。

HostFrame：supervisor到worker的协议帧，私有枚举
    Input(serde_json::Value)
    Response { request_id: String, result: Result<serde_json::Value, ToolError> }


FileInfo：文件信息
    path: LuaString--本次查询项的绝对路径，保留系统路径原始字节
    resolved_path: Option<LuaString>--解析符号链接后的目标路径，不做有损转码
    kind: file | directory | symlink | other--本次查询项自身种类
    size: u64--本次查询项大小，单位为字节
    modified_at_ns: Option<i64>--Unix纳秒修改时间，保留文件系统排序精度；不可取得时None

DirectoryEntry：目录直接子项
    name: LuaString
    FileInfo的全部字段

TextMatch：正则匹配位置
    start: usize--零基字节偏移，包含
    end: usize--字节偏移，不包含

ToolContext：worker私有上下文
    agent_name: String
    turn: String
    source_origin: String--内部Lua程序来源，public_value不暴露该键
    task_id: String
    call_id: String
    alias: String--本次被调用的入口alias，不一定是原注册入口
    owner_task_id: Option<String>--从Tools记录传入，值由注入时显式配置；调用参数不能覆盖
    binding: Option<serde_json::Value>--脚本自定义不透明数据，宿主保存并原样注入
    resource_dir: PathBuf--当前工具资源目录，供定位同目录bin；不从arguments派生
    project_root: PathBuf
    temp_dir: PathBuf
    public_value(&self, arguments: &serde_json::Value) -> Result<serde_json::Value, Error>
        行为：
            生成只读公共上下文，kind=tool、agent_info.name为真实名称，arguments来自本次调用
            隐藏内部agent_name与source_origin平铺键，保留调用参数与真实路径材料
```

# types pub

## 类型

公开：
```text
ToolTaskHandle：可克隆的任务共享句柄，公开类型
    inner: Arc<ToolTaskShared>
    updates: TaskUpdateContext--来源于同一ToolTasks，source.prepare只捕获集合Weak，避免强引用环
    write / replace / try_return / set_error / complete_cleanup
        行为：只构造具体修改闭包交给update_task；取消/超时/worker故障/中止与正常执行不能旁路。
    is_cleanup_complete(&self) -> bool
        行为：
            仅内部清理等待者查询清理是否完成，不提供对外completed状态或完成缓存。
    snapshot(&self, task_id: &str) -> Option<ToolTaskSnapshot>
        行为：在任务shared短锁中原子读取call_id、alias、returned、output及error，cleanup_complete则None；不改变状态和脚本读游标。
    impl Clone
        行为：
            共享同一任务状态、输入与取消能力，不复制任务。

TaskSetShared
    entries: Mutex<BTreeMap<String, ToolTaskHandle>>
    gate: Arc<Mutex<()>>
    revision: Arc<AtomicU64>
    retire: Mutex<Option<Box<dyn Fn() + Send + Sync>>>
    impl Drop：实际集合释放时关闭同步来源，回调不强引用集合。
TaskUpdateContext
    owner: Entity
    gate: Arc<Mutex<()>>
    revision: Arc<AtomicU64>
    sync: SyncContext<AgentTaskSnapshot>
    same_owner(&self, other: &Self) -> bool
    commit(&self) -> u64
    publish(&self, revision: u64)
    close(&self)
TaskSyncBinding
    clients: Clients
    targets: Arc<[ClientSelector]>
    encode: Arc<dyn Fn(AgentTaskSnapshot) -> Result<Arc<ClientFrame>, ClientPrepareError> + Send + Sync>
    new(clients, targets, encode) -> Self
    bind(&self, owner: Entity, address: String, state: Weak<TaskSetShared>) -> TaskUpdateContext
        行为：初始化source，快照读取升级Weak并取gate，返回真实revision及非破坏性完整任务窗口。
AgentTaskSnapshot
    address: String
    tasks: Vec<ToolTaskSnapshot>

ToolTaskShared：任务持有数据
    cancel: CancellationToken--存活任务取消能力；回合中止只触发原子裁决仍未return的任务，不取消已return后台任务
    task: Mutex<Option<JoinHandle<()>>>--监督任务句柄，取消时不直接abort
    shared: Mutex<ToolTaskData>--私有；只允许update_task修改同步可见字段，returned与唯一reply同锁裁决
    input: mpsc::Sender<serde_json::Value>

ToolTaskData：任务查询数据
    output: Vec<serde_json::Value>
    cursor: usize
    replaced: bool--上次查询后是否发生整体替换，初始false
    call_id: String--正常及强制return使用相同调用关联
    alias: String--正常及强制return使用相同工具入口
    returned: bool--LLM结果是否已交付；finish不是可存储的任务状态
    reply: Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>--仅ToolTaskData持有
    error: Option<ToolError>

ToolTaskSnapshot：活跃任务的非破坏性只读快照，公开纯值类型
    task_id: String
    call_id: String
    alias: String
    returned: bool
    output: Vec<serde_json::Value>--当前保留的完整结构化输出窗口，不是cursor之后的增量
    error: Option<ToolError>

TaskReadResult：增量查询结果
    replaced: bool--true表示本次内容替换此前展示，false表示追加
    output: Vec<serde_json::Value>
    error: Option<ToolError>--仅活任务；finish后查询TaskMissing

ToolExecuteResponse：调用结果，公开类型
    call_id: String
    alias: String
    content: String
    images: Vec<ToolImage>
    failed: bool

ToolImage：工具图片，公开类型
    media_type: String
    data: String--标准base64

ToolExecutionLimits：宿主资源配置
    lua_vm_memory_bytes: usize--单工具Lua VM分配器上限，不限制Rust或外部进程
    cache_bytes: usize--每任务输出缓冲滚动窗口，不限制task_return结果
    validate(&self) -> Result<(), ToolError>
        行为：
            要求两项均大于零，由compose将tools.lua_vm_memory_limit和tools.task_output_buffer_window覆盖工具默认值后注入
            宿主不设置输入输出、图片、深度、指令数和队列数量配额
            通过Lua::set_memory_limit限制同一个工具Lua状态，覆盖顶层加载、configure及execute
            helper、Rust缓冲和外部进程不受本配置限额；输出窗口按cache_bytes独立滚动

ToolConfiguration：Lua配置函数返回的本次时间预算
    timeout_ms: u64
    行为：
        timeout_ms为唯一必填字段；正整数为ToolTask存活毫秒数，0不限时。
        拒绝未知字段、负数及不可表示时间，不使用后端默认值。
        ToolTask保存配置确认时确定的deadline；到期取消，等待真实清理后完成。
        不另设宿主、shell或清理限时；仅configure确认后安装执行能力并调用execute。
```

# error

## 类型

公开：
```text
ToolError：工具执行错误
    kind: ToolErrorKind
    message: String
    impl Display + std::error::Error

ToolErrorKind：错误类别，公开枚举
    InvalidRequest | AgentMissing | TaskMissing | DuplicateTask | AliasMissing | InvalidArguments | InvalidDefinition
    ReadFailed | Closed | SubmitFailed | SandboxUnavailable | RunnerStartFailed
    RunnerProtocolFailed | ExecutionFailed | OutputTooLarge | Timeout | Cancelled
    TaskPanicked | CleanupFailed | AlreadyReturned | MissingReturn | InputClosed | InputFull | LogFailed
```

# 持有关系

```text
Agent Entity
├── Agent（agent_core）
├── Tools（tool_resource_core）
├── ToolTasks（tool_core）
│   ├── TaskUpdateContext → 同一Clients Arc槽位
│   └── TaskSetShared → BTreeMap<task_id, ToolTaskHandle>
│       ├── 输出缓存、cursor、return状态与唯一结果回复端
│       └── Lua输入通道
└── Sandboxes与SelectedSandbox（sandbox_resource_core）

mecs全局异步设施
└── supervise_tool
    ├── ToolExecutionContext
    │   └── 与ToolTasks同一原子return及清理能力
    ├── 本次工具及策略材料
    └── runner监督能力
        ├── 可信supervisor
        ├── 受限worker及后代
        └── 管道和临时文件
```
