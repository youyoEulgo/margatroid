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
# types pub tool/core/src/types.rs、lifetime.rs            工具调用、结果及生命周期
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
    observe_tasks: Option<Arc<dyn Fn(Entity, String, &tool_core::ToolTasks) + Send + Sync>>--装配方注入，plugin不依赖dto_plugin
    with_task_observer(self, observe: Arc<dyn Fn(Entity, String, &tool_core::ToolTasks) + Send + Sync>) -> Self
        行为：保存装配方提供的通用集合绑定回调，不引用dto_plugin。
    impl Plugin for ToolPlugin
        build(self, app: &mut App)
            行为：
                require 执行设施可用、预算有效、未安装
                require schedule in [RuntimePlugin::UPDATE, RuntimePlugin::POST_UPDATE]
                install init_tool_tasks_system, tool_execute_system

TaskObserverBinder = Arc<dyn Fn(Entity, String, &tool_core::ToolTasks) + Send + Sync>
ToolRuntime(Arc<WorldClosureSender>)：plugin私有Resource
    行为：插件安装时保存本schedule的World闭包发送能力；owned执行材料只持Weak，避免任务与World强引用环。

```

# components

## 类型

公开：
```text
ToolTasks：通用工具任务句柄集合，tool_core公开组件
    shared: Arc<TaskSetShared>--唯一集合及版本，内部可变字段私有
    updates: TaskUpdateContext--针对集合本身的更新能力，不绑定Entity、地址或客户端
    new() -> Self
        行为：创建accepting=true的空TaskSetShared及其TaskUpdateContext；不要求owner、address或TaskObserverBinder，不挂载组件，不建立网络同步来源。
    close_admission(&self)
        行为：同register的短锁中同步置accepting=false，此后所有Path/Registered登记拒绝，已经登记任务继续清理。
    is_accepting(&self) -> bool
        行为：只读受理门，不判断Entity类型。
    update_context(&self) -> TaskUpdateContext
    snapshot_source(&self) -> Arc<dyn Fn() -> Option<(u64, Vec<ToolTaskSnapshot>)> + Send + Sync>
        行为：返回只捕获集合Weak的读取闭包，在同一更新门读取版本及完整快照；集合释放后返回None。
    observe(&self, changed: TaskChangeObserver, retired: TaskRetireObserver)
        行为：登记集合变更及释放回调；不依赖客户端类型，未登记观察者不影响集合操作。
    register(&self, id: String, handle: ToolTaskHandle) -> Result<(), ToolError>
        行为：调用update_task执行本次登记，同集合短锁检查accepting、唯一task_id及同集合更新身份；关闭后拒绝，不启动未登记监督任务。
    cancel(&self, id: &str) -> Result<(), ToolError>
        行为：
            向对应句柄请求取消，不直接中断监督任务或删除句柄。
    list(&self) -> Vec<(String, ToolTaskHandle)>
        行为：
            return entries中的活任务句柄克隆
    list_returned(&self) -> Vec<TaskListEntry>
        行为：在同一集合更新门内按entries的task_id排序调用handle.list_entry；只保留已returned且未cleanup_complete任务，不复制output，不触发发布、消费或清理。
    snapshot(&self) -> Vec<ToolTaskSnapshot>
        行为：在TaskSetShared更新门内取得当前完整活跃集合，按task_id稳定排序；快照版本与该次读取一致。
            每项包含完整保留窗口；不调用read、不推进cursor、不清除replaced、不发送reply、不触发取消或finish。
            所有可观察更新受同一集合的更新门串行化；释放锁后才编码和投递，不持锁等待IO。
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
    abort_unreturned(&self, turn: &str) -> 工具中止交接结果
        行为：
            for handle in entries where handle.turn == Some(turn):
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

ToolExecuteRequest：单次工具请求，公开事件枚举
    Registered
        id: String
        agent: Entity
        turn: Option<String>--显式回合关联
        alias: String--工具集合中的注册别名
        call_id: String
        arguments: serde_json::Value
        reply: Arc<Mutex<Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>>>
    Path
        initiator: Entity--发起者，只要求实体存在并挂载ToolTasks
        path: PathBuf--实际工具入口文件路径
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
    提交闭包：launch_tool(ToolLaunchGuard) -> Future<Output = Result<ToolLaunchOutput, ToolLaunchError>>
    产出事件：None
    oneshot回复：Result<ToolExecuteResponse, ToolError>
    行为：
        在中止schedule之后执行；
        ToolExecuteRequest -> handle_tool_execute。
        Registered变体逐请求复检原Agent身份、回合及权限；Path变体仅要求initiator实体存在并拥有ToolTasks，不要求Agent、Tools、MclQueue或AgentStorage。
```

# handler

## 函数

crate公开：
```text
handle_init_tool_tasks(world: &mut World, request: InitToolTasks)
    行为：
        认领reply，检查agent实体；
        不存在ToolTasks则用ToolTasks::new()构造并挂载，已存在不覆盖；
        对象地址与网络同步由本plugin的TaskObserverBinder绑定，不交给ToolTasks::new处理；
        组件可查询后回复Ok(())，失败回复ToolError。

handle_tool_execute(world: &mut World, request: ToolExecuteRequest)
    行为：
        按变体分派，认领该变体的原reply。
        Path变体核对完整initiator及ToolTasks受理门，生成实际task_id/execution_id，先同步登记ToolTaskHandle，再提交owned执行闭包；登记失败不启动VM。
            从实际initiator的Workspace或Agent组件取得所属Workspace及工作根；其他仅有ToolTasks的对象可执行不需要工作根的工具，目录相关方法缺能力时明确失败，不以缺少Agent、MCL或AgentStorage拒绝通用调用。
            固定规范tool_path及原始arguments到宿主ToolExecutionContext，读取和运行在worker中执行，参数副本及Lua context修改不能改变这些材料。
            若有实际Address和observe_tasks，则登记后绑定该集合的公共来源；无地址的通用执行对象不伪造网络地址。
            原reply只有工具监督拥有，task_id及call_id内部生成；Path结果call_id为实际task_id，alias为工具路径，不追加请求字段。
            错误日志无绑定存储时走tracing；有AgentStorage时使用实际克隆写入，但在所绑定Agent关闭前释放诊断引用，关闭后只用tracing。
        Registered变体核对Agent完整Entity、实际Agent.instance、调用身份与权限、取消状态及任务受理组件；Closing拒绝。
        turn=None为自由调用，Starting/Running可受理；省略不自动继承当前Agent或调用者turn，初始化工具无需伪造回合。
        turn=Some(id)必须非空且Agent为Running、当前有效turn与id匹配；失效或不匹配返回保留call_id/alias的中止工具结果，不注册任务、不启动闭包或进程。
        Registered变体entry = Tools.get(request.alias) or return 对应工具错误
        使用entry.input_schema校验arguments
        按entry.path执行；
        owner_task_id与binding放入context
        Registered变体从SelectedSandbox及Sandboxes取得可选策略；Path仅使用发起者已具备的策略，无此组件时不附加策略，不扩张进程权限。
        模型参数不能覆盖宿主context标记
        固定本次工具、策略路径和目录参数，准备owned执行材料，不跨await借用World。
        受理后ToolTask独立持有执行材料；之后Tools记录被删除不影响该任务。
        在plugin构造独立cleanup异步锁、cleanup_proven及TaskFinish回调，从ToolTasks.updates衍生同一来源的ToolTaskHandle，经统一Register入口登记，再把该能力克隆交给工作任务；
        通过send_async_closure提交launch_tool，失败通过原reply回复错误；
        不缓存执行器Handle。
```

私有：
```text
update_task<R>(context: &TaskUpdateContext, change: impl FnOnce() -> Result<(R, bool), ToolError>) -> Result<R, ToolError>
    行为：取得必需context.gate，在同一更新门执行具体修改，失败不发布；返回的changed仅在可观察状态实际改变时为true。同门内递增revision，释放门后调用已登记的通用变更回调；客户端投递由plugin绑定的回调负责。
    生产唯一写能力由ToolTaskHandle/ToolTasks方法构造具体闭包，内部可变字段保持私有；网络准备只持Weak集合，读取完整快照时取得同一更新门。

launch_tool(launch: ToolLaunchGuard) -> Future<Output = Result<ToolLaunchOutput, ToolLaunchError>>
    行为：
        从ToolLaunchGuard取得context槽位，tokio::spawn监督任务先等私有start oneshot；先把真实JoinHandle attach到已登记ToolTaskHandle，再释放start，最后解除guard。
        attach/提交失败由ToolLaunchFallback保留原context和reply收口；成功返回无字段ToolLaunchOutput，仅表示交接。supervise_tool负责实际结果和执行清理。

supervise_tool(context: ToolExecutionContext) -> Future<Output = ()>
    行为：
        blocking worker读取程序；Registered变体使用受理时固定的ToolEntry及Schema，Path变体读取实际入口的Tool定义、Schema及程序，不依赖Tools记录；
        sandbox_resource_core::read_sandbox读取策略，不重新读取工具定义。
        校验arguments与本入口的有效schema、具体后端策略；
        检查本任务取消状态后启动run_runner，动态入口不读取注册schema作回退。配置及execute收到相同可信owner_task_id/binding/resource_dir，不接受arguments覆盖。
        接收runner的Return时通过共享句柄的原子return裁决交付结果，不结束runner；
        中止、错误与正常Return共用唯一回复端，不能在context另持独立可回复状态；
        缓存写入、覆盖、正常/强制Return、错误及清理移除均调用update_task；任务输入与脚本read只保留各自原语义。
        自然退出/finish/到期/中止:
            TaskScope.close取出并调用工具注册撤销句柄；各动作排入World按alias及RegistrationId注销
            先等待所有已受理agent_create的创建交接完成，再调用ToolTaskHandle.finish_children()等待子Agent实际关闭；子关闭不发送StopAgent、不反向finish本任务
            注销不等待或取消其他已受理ToolTask；cleanup_owner等待同一World发送器上的后续屏障，确认之前撤销操作已处理
            未return则交付对应失败结果
            await VM、runner、后代、管道、临时文件真实清理
            调用统一CompleteCleanup入口移除本任务并释放缓存，触发通用集合变更回调
        已回复调用不得重复回复；
        后台异常记录于对应任务error，未回复调用按实际错误完成。
        清理失败及时交付CleanupFailed；
        保持未finish并保留本次runner管理能力继续清理，不标记已finish。

supervise_tool执行结束收口
    行为：受保护执行Future正常返回代表runner和临时目录清理路径已真实结束，即使业务结果为Err；panic被转换为CleanupFailed且不置cleanup_proven。
        完成cleanup_owner后才设置context.cleanup_proven=true，drop执行材料并请求handle.finish；TaskFinish异步等待本监督真正退出，不循环等待自己。

run_runner(context: &mut ToolExecutionContext, request: RunnerRequest, policy: Option<Value>) -> Future<Output = Result<(), ToolError>>
    行为：
        直接启动可信supervisor，传递执行材料及可选沙箱后端与策略。
        不建立内存cgroup、systemd scope或全局MemoryPool，不设置外部进程内存额度。
        独立writer任务串行写RunnerRequest和HostFrame，diagnostic_drain独立读取stderr；主select循环持续读取stdout及任务输入。
        RunnerFrame::Host立即加入FuturesUnordered pending_hosts，响应完成后经writer发送；不在stdout读取分支串行await Host调用，避免子Agent关闭期间停止泵送清理消息。
        主循环同时受cancel及lifetime_expired驱动；pending Host调用的等待取消不会取消create_agent已独立spawn的创建监督，任务finish_task仍等真实创建watch。
        取消或超时向supervisor发送终止请求，继续等待真实后代回收证明、supervisor退出和两端管道收口。
        清理失败保持监督，不以丢弃Future或收到Return冒充清理成功。
        Return不是进程终态；Stopped(cleanup_proven=true)、child.wait及writer/diagnostics实际join全部完成才作为执行清理证明。无证明时持续保留监督，不complete_cleanup。

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
        创建执行级HttpContext并用lua_runtime_core共享install_http绑定http_request；HTTP在当前受沙箱约束的worker直接执行，不通过HostFrame/LuaServices转发到权限更大的daemon。
        HttpContext使用当前执行cancel和整体deadline；worker自然退出先close_and_wait释放在途请求流，再进入原进程与任务清理。worker被强制终止时由OS关闭socket，父监督仍等待原worker真实退出。
        无新增System、事件或进程协议，HTTP的输入、响应、固定预算与错误定义使用Lua Runtime设计，不复制另一套HTTP实现。
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
    http: 通用HTTP子对象--request，execute阶段开放；实现和类型由lua_runtime_core共享
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

margatroid.data:image_prepare(bytes: LuaString, options?: ImagePrepareOptions) -> ImagePreview
    行为：复用lua_runtime_core的data::install纯Rust图片预览转换，完整类型与算法见Lua Runtime设计；不增加工具专用helper或图片进程。
        read-image资源不再按原文件5MiB拒绝输入，先解码并压缩，再交付预览；默认max_dimension=1280、target_bytes=307200、quality=80，可由本次工具入参显式调整。
        返回的图片结果使用预览字节而非原始字节，正文记录原尺寸与预览尺寸、编码和大小；原文件与已有消息历史不改。

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

margatroid.mcl:execute(target: String, instruction: String, arguments: Option<Vec<Value>>) -> Future<Value>
    行为：工具worker通过RunnerFrame::Host转发公共服务，宿主绑定实际ToolTaskHandle.execution_id及取消能力；与自动Lua使用同一LuaServices，原MCL队列/审计/oneshot保留。
        mcl_value把MclValue::Unit显式投影为空JSON对象{}，其他成功值保持原结构，MCL Error保持{Error=...}；不以nil/null表示无数据成功。
margatroid.agent:stop(target: String) -> Future<Value>
    行为：核对实际来源，调用agent_core::StopAgent定位目标创建任务；使用独立受监督请求并在调用者任务之外等待原reply，不能让目标Driver的execute等待自身所属任务finish。
        Lua返回{}仅表示关闭请求已接受，任务最终清理仍由外部监督确认；网络或外部事件调用者可直接等待StopAgent.reply的实际完成。

margatroid:wait(target: String) -> Future<()>
    行为：在明确Agent上等待一次唤醒，与自动Lua相同，不把等待目标作为VM所有者。
margatroid.agent:create({node_name: Option<String>, alias: Option<String>, project_root: String, config_path: String, ui_path: String}) -> Future<Value>
    行为：从ToolExecutionContext取得实际Workspace、initiator、task_id、规范tool_path和原始arguments；不读取Lua可修改context认定来源。
        node_name、alias和工作目录是Driver专用参数，来自工具arguments及Driver实际create调用；若arguments声明node_name，create调用必须相同，不能用参数nodeA而实际创建nodeB；缺省节点名为临时Agent。
        取得WorkspaceDatabase.try_operation的节点凭证；普通节点检查原Driver及parent_node，其他Driver或其他父节点立即拒绝，正在创建/归档返回Busy。已有运行Agent或Closing实体也拒绝。
        调用prepare_agent取得原记录或新候选记录，决定完整节点为实际initiator地址加runtime_name；无节点名使用~temporary-<id>，不伪造父节点实体。
        在blocking worker中准备agents/normal/<node_name>或agents/temporary/<id>；数据库为state.sqlite、配置为setting.toml、Block布局沿storage_core；新目录独占创建，已有记录复用其目录。已有配置不覆盖，缺失配置从config_path原文复制并通用TOML语法校验。
        校验Driver提供的project_root是现有绝对目录、位于实际initiator工作范围且真实symlink不越界；框架不解释或拼接arguments.path。
        同一worker调用storage_core::open_storage打开该Agent数据库，取得已连接可用的AgentStorage，再调用commit_agent保存可信Driver路径和完整tool_arguments，确认提交才通知目录变化。
        发送agent_core::StartAgent<AgentStorage>，传workspace、parent=initiator、task_id、完整节点、project_root、ui_path、已连接句柄及原创建reply；不重复启动Driver。
        独立创建监督持有节点凭证、存储及StartAgent接收端，等待AgentHandle返回，确认System已绑定实际创建任务及完成首次来源安装；成功向Lua返回{address: String}，AgentHandle仅由宿主及父任务保存。
        Lua调用者取消或任务关闭时创建监督仍继续：失败在blocking worker关闭已打开存储；若agents记录尚未提交，只回收本次独占创建的目录，已有记录的目录不删，回收失败保留诊断路径；成功则调用AgentHandle.close并等待，最后释放凭证和创建完成观察，不返回可用Agent、不遗留实体。
        任何提交前失败先关闭本次部分连接，再回收本次独占目录；旧记录目录不删。
        若close_storage或本次目录回收失败，把仍持有存储、已验证目录及operation Arc的重试回调交给creator_task.retain_cleanup；创建Completion置settled错误，不能把它当作资源已清理。
        父任务继续执行retry_cleanups，只有回调成功后释放最后的operation Arc并允许CompleteCleanup；创建监督本身可退出，但不能丢失连接、凭证或只返回错误就让Workspace删除数据。
        提交结果不确定时先核验agents中本次id，无法确认则保留文件及已打开能力的关闭监督并返回错误，不擅自删除可能已登记的数据。
        记录确认提交后实体创建失败保留该记录和已准备文件供Existing重试，返回带registered=true及phase的错误；提交前失败不发布新记录，未登记残留目录不被另一个Driver直接复用。

margatroid.tools:register(registration: RegisterToolsRequest) -> Future<Output = ()>
    行为：仅工具execute可用。注册批次指定目标Agent，每项lifetime独立指定任务；缺省Agent为当前Agent，缺省lifetime不绑定发起任务。
        inject_tools在World闭包校验实际发起任务身份和同Workspace当前目标，原子登记完整批次；程序路径固定为当前工具资源，不接受参数指定任意程序。
        绑定任务关闭时只撤销该批次对应注册，不取消已受理调用，不增加同步事件或监听任务。
        MCP资源用同一脚本的binding.kind区分mcp_tool和mcp_update，无binding执行首次注册；任务id和远端入口由可信context提供，不进入模型参数。
        业务与更新定义均可绑定同一服务任务；已受理的更新执行独立于该归属，关闭旧服务后仍能继续注册新定义并使自身成为服务任务，宿主不增加MCP专用分流或持久化接口。
margatroid.workspace:agents() -> Future<Output = Vec<Value>>
    行为：工具execute异步请求WorkspaceAgents，宿主核对实际调用任务后实时查询同Workspace成员address/name/state，不缓存启动时成员列表。
inject_tools(context: &ToolExecutionContext, registration: RegisterToolsRequest) -> Future<Output = Result<(), ToolError>>
    行为：逐项解析lifetime为实际TaskHandle，以其TaskScope.bind创建撤销句柄；动作捕获Weak World发送能力、完整目标Entity、alias及RegistrationId。
        Tools.insert_registered_batch先完整校验并核对绑定可用性，再同时提交定义和凭证并发布一次；任一失败释放已建立凭证，目标无部分工具。
cleanup_owner(context: &ToolExecutionContext) -> Future<Output = Result<(), ToolError>>
    行为：handle.cancel关闭注册作用域并请求执行取消；在同一World队列提交屏障并等待，不扫描Tools。
resolve_agent(world: &World, source: Entity, address: Option<&str>) -> Result<Entity, ToolError>
    行为：规范化公开Address，定位完整当前Entity，核对其Agent和source同Workspace且非Closing；无address使用source。
ensure_invocation(world: &World, agent: Entity, task_id: &str, expected: &ToolTaskHandle) -> Result<(), ToolError>
    行为：核对任务表实际句柄same_task且未取消/未清理，防止旧worker借同名新任务执行操作。
task_target(world: &World, source: Entity, target: &TaskReference) -> Result<ToolTaskHandle, ToolError>
workspace_agents(context: &ToolExecutionContext) -> Future<Output = Result<Value, ToolError>>
    行为：通过既有World闭包获取当前成员；不发事件、不创建独立监控。
task_reference(lua: &Lua, value: LuaValue) -> mlua::Result<TaskReference>
    行为：字符串为当前Agent任务id；对象严格读取TaskReference并规范化公开Agent地址。

margatroid.task:append_output(value: LuaValue) -> Future<Output = ()>
    行为：
        仅execute阶段可用，向当前任务缓冲追加值
        窗口超限滚出旧项并调整cursor；超长字符串保留UTF-8尾部，放不下的结构项整项滚出

margatroid.task:replace_output(value: LuaValue) -> Future<Output = ()>
    行为：
        仅execute阶段可用，替换当前输出缓冲，cursor=0且replaced=true
        替换与读取共用短锁，不改变任务存活状态

margatroid.task:list() -> Future<Output = Vec<TaskListEntry>>
    行为：严格无参、execute阶段开放；经既有task_list公共宿主回调核对执行身份，只从当前调用Agent的ToolTasks读取list_returned。
        无实际任务集合明确失败，不跨Agent或Workspace查询；当前任务读取时尚未return，自动不列出。空数组保留JSON数组身份。
        不读取原调用参数或缓存正文、不推进读取cursor、清除replaced、增加revision或发送sync。

margatroid.task:read_output(target: TaskReference | String) -> Future<Output = TaskReadResult>
    行为：
        仅execute阶段可用，按结构化目标读取同Workspace实际任务的增量缓冲并推进cursor
        不返回completed字段，已经finish的目标返回TaskMissing

margatroid.task:return_result(value: LuaValue) -> Future<Output = ()>
    行为：
        仅execute阶段可用，原子交付原调用一次结果，不结束VM或关闭输入
        与中止共用原reply裁决，重复回复报错，成功交付后异常仍只记录日志

margatroid.task:finish(target: TaskReference | String) -> Future<Output = ()>
    行为：
        仅execute阶段可用；结构化引用支持同Workspace任务，省略Agent时选择当前执行任务集合
        自身以控制退出结束Lua，由监督者真实清理；不等待自己的JoinHandle
        其他任务在真实清理并移除后才返回成功，已finish目标返回TaskMissing

margatroid.task:send(target: TaskReference | String, value: LuaValue) -> Future<Output = ()>
    行为：
        仅execute阶段可用，通过真实TaskReference定位同Workspace任务并向其输入通道投递结构化值；省略Agent使用当前执行任务集合
        成功仅表示已入队，不表示目标已经处理

margatroid.task:receive() -> Future<Output = LuaValue>
    行为：
        仅execute阶段可用，等待当前任务的一条输入，不持缓存锁
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

ToolInvocation：plugin内部Clone值
    id: String
    agent: Entity--Path为initiator，Registered为目标Agent
    turn: Option<String>
    alias: String
    call_id: String
    arguments: Value
ToolExecutionContext：plugin内部owned材料
    request: ToolInvocation
    storage: Option<storage_core::AgentStorage>
    agent_name: String
    agent_address: String
    workspace: Value
    diagnostic_received: bool
    path_entry: bool
    cleanup_proven: Arc<AtomicBool>
    trusted_path: Arc<Mutex<PathBuf>>
    tool_entry: tool_resource_core::ToolEntry
    world: Weak<WorldClosureSender>
    sandbox_path: Option<PathBuf>
    project_root: PathBuf--缺少真实工作根时为空路径，不制造Agent身份
    runner_path: PathBuf
    limits: ToolExecutionLimits
    configuration: Option<ToolConfiguration>
    handle: ToolTaskHandle
    tasks: ToolTasks
    input: mpsc::UnboundedReceiver<Value>
TaskFinish注入闭包：每次执行的独立清理能力
    cleanup: Arc<tokio::sync::Mutex<()>>--串行化显式finish重试
    cleanup_proven: Arc<AtomicBool>
    tasks: ToolTasks
    id: String
    world: Weak<WorldClosureSender>
    行为：收到handle立即创建oneshot，将清理Future通过World闭包取得AsyncRuntimeHandle后spawn；等待者Future只等原oneshot，取消等待不取消监督。
        持cleanup异步锁，已清理直接成功，否则等handle.wait_execution_ended并要求cleanup_proven；无runner清理证明返回CleanupFailed并保留任务。
        调用services::finish_task等待在途Agent创建并释放诊断/调用登记，再finish_children及retry_cleanups，全部成功才tasks.complete_cleanup。
        清理失败set_error并回复；下一次finish重新执行尚未完成清理，不缓存失败结果，不重跑工具业务。
ToolContext：发送给工具worker的纯值
    agent_name: String
    agent_address: String
    workspace: Value
    owner_agent: Option<String>
    turn: Option<String>
    source_origin: String
    task_id: String
    call_id: String
    alias: String
    owner_task_id: Option<String>
    binding: Option<Value>
    resource_dir: PathBuf
    project_root: PathBuf
    temp_dir: PathBuf
RunnerRequest
    protocol_version: u32
    program: String
    arguments: Value
    context: ToolContext
    limits: ToolExecutionLimits
ToolLaunchOutput：无字段
ToolLaunchError
    fallback: Option<ToolLaunchFallback>
    error: ToolError
ToolLaunchFallback：Clone
    slot: Arc<Mutex<Option<ToolExecutionContext>>>
    handle: ToolTaskHandle
    fail(&self, failure: ToolError)
        行为：未交接context仍在slot时认领原reply、确认未启动清理并complete_cleanup；已交接则只return_result/set_error/cancel，保留实际监督。
ToolLaunchGuard
    fallback: ToolLaunchFallback
    armed: bool
    impl Drop：armed时调用fallback.fail(SubmitFailed)，不丢失尚未交接context。
RunnerFrame::Host { request_id: String, method: String, arguments: Value }
    行为：工具bin复用公共Lua宿主服务协议，宿主自动绑定实际任务身份，不接受脚本伪造source。
LuaExecutionTask（tool_core公开组件）
    workspace: Entity
    handle: ToolTaskHandle
    tasks: ToolTasks
    input: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Value>>>
    resource_dir: PathBuf--自动任务的真实Lua入口路径
    行为：复用原任务scope/输入/输出/Return能力，创建的Agent通过同一任务scope登记清理责任；对象与网络同步由plugin绑定，不在core构造时要求Agent身份。
LuaServices安装（tool/plugin/src/services.rs）
    install(app: &mut App)
    install_on(app: &mut App, schedule: &str)
        行为：应用内唯一服务表，Weak World发送能力避免环；自动执行和tool bin转发使用同一invoke。
    CreateArguments：Deserialize，deny_unknown_fields
        node_name: Option<String>
        alias: Option<String>
        project_root: PathBuf
        config_path: PathBuf
        ui_path: PathBuf
    Invocation：Clone
        task_id: String
        path: Arc<Mutex<PathBuf>>--实际入口规范化后更新到同一可信来源
        arguments: Value
    ExecutionBindings：Resource
        invocations: Mutex<HashMap<String, Invocation>>
        diagnostics: Arc<Mutex<HashMap<String, BTreeMap<u64, MonitoredAgent>>>>
        creating: Mutex<HashMap<String, Vec<watch::Receiver<bool>>>>
    MonitoredAgent
        entity: Entity
        session: String--值为Agent.instance，不是AgentSession组件
        storage: AgentStorage
    CreationDone(watch::Sender<bool>)
        impl Drop：独立创建监督真实退出才send_replace(true)，Lua Future取消不代表创建结束。
    PreparedCreation
        database: workspace_core::WorkspaceDatabase
        operation: Arc<workspace_core::WorkspaceOperation>
        record: workspace_core::WorkspaceAgent
        creator: ToolTaskHandle
        task_id: String
        parent: Entity
        root: PathBuf
    PendingAgentHandoff：Clone
        world: Weak<WorldClosureSender>
        address: String
        parent: Entity
        task_id: String
        storage: AgentStorage
    Compensation
        pending_handoff: Option<PendingAgentHandoff>
        agent: Option<AgentHandle>
        storage: Option<AgentStorage>
        directory: PathBuf
        remove_directory: bool
        operation: Option<Arc<workspace_core::WorkspaceOperation>>
    create_agent(sender: Arc<WorldClosureSender>, identity: LuaExecutionIdentity, arguments: Value) -> Future<Output = Result<Value, LuaRuntimeError>>
        行为：World校验真实调用并登记creating的watch，然后tokio::spawn独立create_supervised；调用者只await原oneshot。
    create_supervised(sender: Arc<WorldClosureSender>, identity: LuaExecutionIdentity, args: CreateArguments, invocation: Invocation) -> Future<Output = Result<Value, LuaRuntimeError>>
        行为：取得节点操作凭证和候选记录，spawn_blocking准备文件、open_storage连接及commit_agent；再经World发送StartAgent等待原reply。
            System已在回复前bind_child；交接后核对Agent.parent/task_id及创建者仍存活，登记真实存储诊断并返回{address}。
            取消/错误沿compensate收口；失败把同一Compensation闭包retain_cleanup到创建任务，保留连接和operation供重试。
    compensate(state: Arc<Mutex<Compensation>>) -> Future<Output = Result<(), ToolError>>
        行为：失联StartAgent交接先在World按same_database找真实实体并核对父任务/地址，使用其保留的child_close等待；不直接关闭仍被实体持有的数据库。
            关闭已取得AgentHandle后，blocking worker关闭storage、只删除本次独占未登记目录，最后释放operation；失败保留原能力。
    finish_task(sender: Arc<WorldClosureSender>, handle: ToolTaskHandle) -> Future<Output = Result<(), ToolError>>
        行为：取走该execution_id的creating列表逐个等待真实done，再移除诊断和invocations，不用旧创建失败永久阻止cleanup。
    finish_execution(sender: Arc<WorldClosureSender>, identity: LuaExecutionIdentity, diagnostic: Value) -> Future<Output = Result<Value, LuaRuntimeError>>
        行为：定位实际任务，先finish_task，再close_registrations及finish_children；不发送StopAgent关闭本任务。
    ToolContext::public_value(&self, arguments: &Value) -> Result<Value, serde_json::Error>
        行为：序列化独立context.arguments，移除source_origin、temp_dir和agent_name；只对实际Agent附agent_info，缺失工作根或Workspace时省略相应字段。
    invoke(sender, identity, method, arguments) -> LuaServiceFuture
        行为：所有实际World读取/修改处validate_execution核对source Address、实际VM或唯一task-instance及取消状态。
        execution_log将自动执行的主动日志投递给实际绑定Agent存储，沿原提交探针发布；无Agent的诊断由Lua宿主直接按原级别输出tracing，受既有日志级别及过滤配置控制。
        agent_data_dir要求Agent Address字符串，validate_execution后定位同Workspace真实Agent，返回其AgentStorage::data_dir字符串；目标不存在、跨Workspace或未挂载storage明确失败，不退回资源/工作目录、不创建目录。
        agent_stop提交原StopAgent并在调用栈外监督原reply；
        agent_create接收node_name/alias/project_root/config_path/ui_path，所属Workspace及Driver路径由宿主持有的实际执行身份取得，调用上述创建注入方法完成登记或同Driver复用；不同Driver不得接管已有节点，持久目录按普通、临时、归档类别区分。
        agent_create先登记在途创建完成观察，再按上述受监督准备/连接/提交/创建顺序执行；取消后仍等待原reply并通过返回AgentHandle清理。
        mcl_execute沿MclCommand原reply，wait从目标agent_core::Agent.wake取得通用槽位，不引用旧AgentWaitSlot；所有宿主服务继续校验真实任务和取消状态。

```

# types pub

## 类型

公开：
```text
ToolTaskHandle：可克隆的任务共享句柄，公开类型
    turn(&self) -> Option<&str>--读取任务不可变回合关联；None不归属于任何用户回合
    new(limits: &ToolExecutionLimits, call_id: String, alias: String, reply: oneshot::Sender<Result<ToolExecuteResponse, ToolError>>, updates: TaskUpdateContext, turn: Option<String>, finish: TaskFinish) -> Result<(Self, mpsc::UnboundedReceiver<Value>), ToolError>
    execution_id(&self) -> &str--每个实际任务唯一，MCL来源不使用可重用业务id
    inner: Arc<ToolTaskShared>
    updates: TaskUpdateContext--来源于同一ToolTasks；plugin的快照读取能力只捕获集合Weak，避免强引用环
    write / replace / return_result / set_error
        行为：只构造具体修改闭包交给update_task；取消/超时/worker故障/中止与正常执行不能旁路。
    finish(&self) -> Future<Output = Result<(), ToolError>>
        行为：立即请求取消，由plugin注入的独立清理监督执行；调用者Future只等待，不持有清理执行权。并发finish通过plugin清理异步锁串行复核实际完成结果，失败可再次显式finish重试未完成清理，已退出Lua/进程不重跑。
            等待创建监督、finish_children及所有资源收口后才CompleteCleanup；父任务完成必须晚于全部子Agent关闭。该能力在register前已安装，不存在已登记而无人监督的任务。
    is_cleanup_complete(&self) -> bool
        行为：
            仅内部清理等待者查询清理是否完成，不提供对外completed状态或完成缓存。
    list_entry(&self, task_id: &str) -> Option<TaskListEntry>
        行为：持任务shared短锁检查returned且非cleanup_complete，仅复制id、alias到tool_name及error；has_unread_output为replaced或cursor小于output长度，不克隆缓存、推进cursor或清除replaced。
    snapshot(&self, task_id: &str) -> Option<ToolTaskSnapshot>
        行为：在任务shared短锁中原子读取call_id、alias、returned、output及error，cleanup_complete则None；不改变状态和脚本读游标。
    impl Clone
        行为：
            共享同一任务状态、输入与取消能力，不复制任务。

TaskSetShared
    entries: Mutex<BTreeMap<String, ToolTaskHandle>>
    accepting: AtomicBool--register与close_admission共享更新门裁决
    gate: Arc<Mutex<()>>
    revision: Arc<AtomicU64>
    retire: Mutex<Option<TaskRetireObserver>>
    impl Drop：实际集合释放时调用已登记的retire回调，回调不强引用集合。
TaskChangeObserver = Arc<dyn Fn(u64) + Send + Sync>
TaskRetireObserver = Box<dyn Fn() + Send + Sync>
TaskUpdateContext
    gate: Arc<Mutex<()>>
    revision: Arc<AtomicU64>
    changed: Arc<Mutex<Option<TaskChangeObserver>>>
    same_collection(&self, other: &Self) -> bool
        行为：比较集合更新门的共享身份，不通过Entity或地址判断。
    commit(&self) -> u64
    publish(&self, revision: u64)
        行为：调用已登记的变更回调；未登记时不执行投递，不影响本次修改。
AgentTaskSnapshot
    address: String
    tasks: Vec<ToolTaskSnapshot>

ToolTaskShared：任务持有数据
    execution_id: String
    turn: Option<String>--构造时固定的显式回合关联
    registrations: TaskScope
    finish: TaskFinish--plugin注入的清理请求，回调不捕获本任务强引用
    children: Mutex<TaskChildren>--同时保存子Agent与可重试补偿
    child_cleanup: tokio::sync::Mutex<()>
    retry_cleanup: tokio::sync::Mutex<()>
    task_attached: AtomicBool
    supervisor_ended: AtomicBool
    changed: Notify
    cache_bytes: usize
    cancel: CancellationToken--存活任务取消能力；回合中止只触发原子裁决仍未return的任务，不取消已return后台任务
    task: Mutex<Option<JoinHandle<()>>>--监督任务句柄，取消时不直接abort
    shared: Mutex<ToolTaskData>--私有；只允许update_task修改同步可见字段，returned与唯一reply同锁裁决
    input: mpsc::UnboundedSender<serde_json::Value>

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

TaskListEntry：已return未finish任务的轻量纯值
    task_id: String
    tool_name: String--创建任务时实际工具别名
    has_unread_output: bool--不表示idle或完成
    error: Option<ToolError>--JSON必有nullable
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
ToolTaskHandle::retain_cleanup(&self, close: TaskChildClose) -> Result<(), ToolError>
    行为：同清理门下检查尚未CompleteCleanup，登记可重试补偿；回调不能捕获本任务强引用。已登记创建监督结束前任务不可能完成，故可在取消后接收补偿。
ToolTaskHandle::retry_cleanups(&self) -> Future<Output = Result<(), ToolError>>
    行为：克隆id/回调后释放短锁，执行全部补偿，成功项按id移除、失败项保留；全部结果实际返回后再聚合错误，不持锁跨await。
    CompleteCleanup要求创建已settled、全部子Agent已关闭且cleanup_retries为空，否则拒绝移除任务。

TaskChildCloseFuture = Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send>>
TaskFinish = Arc<dyn Fn(ToolTaskHandle) -> TaskChildCloseFuture + Send + Sync>
    行为：core把本次实际句柄传给plugin回调；回调立即请求独立清理监督并返回结果等待，不把调用者Future作为清理执行载体。
TaskChildClose = Arc<dyn Fn() -> TaskChildCloseFuture + Send + Sync>
ToolTaskHandle::bind_child(&self, entity: Entity, close: TaskChildClose) -> Result<(), ToolError>
    行为：在任务短锁核对仍接受子绑定，按完整Entity登记通用关闭回调；不引用agent_core类型。已取消/关闭时拒绝，调用方关闭刚创建实例并等待。
ToolTaskHandle::retain_child_for_cleanup(&self, entity: Entity, close: TaskChildClose) -> Result<(), ToolError>
    行为：同children锁登记创建失败的补偿关闭能力；允许任务已取消或普通绑定已关闭，但必须尚未CompleteCleanup。
        只由已登记的在途创建监督使用；任务须先等该创建交接结束，因此正常调用链不可能在本登记之前移除任务。登记不返回可用Agent，不重新打开子创建受理。
ToolTaskHandle::finish_children(&self) -> Future<Output = Result<(), ToolError>>
    行为：关闭子绑定受理，取本任务全部子关闭回调，请求关闭并等待所有结果；失败项保留供显式finish重试，已成功项移除。
    正在创建的Agent须先由finish_execution等待创建交接；本方法不持锁跨await，不把同步scope撤销视为Agent已关闭。
TaskChildren：core私有共享值
    accepting: bool
    completed: bool
    children: BTreeMap<Entity, TaskChildClose>
    retries: BTreeMap<u64, TaskChildClose>
    next_retry: u64

TaskScope(Arc<ScopeInner>)：Clone + Default + Debug
    bind(&self, revoke: impl FnOnce() + Send + 'static) -> Result<TaskBinding, ToolError>
        行为：同一短锁检查closed及登记动作；不存在轮询或常驻等待任务。
    close(&self)
        行为：锁内一次关闭并take登记集合，锁外调用撤销动作。
    availability(&self) -> Box<dyn Fn() -> bool + Send + Sync>
        行为：只捕获Weak作用域，查询时检查实际任务仍存在且未关闭；不负责清理。
    is_closed(&self) -> bool
ScopeInner
    state: Mutex<ScopeState>
    impl Drop：实际任务共享对象释放的兜底路径调用剩余撤销动作。
ScopeState
    closed: bool
    registrations: BTreeMap<u64, Box<dyn FnOnce() + Send>>
TaskBinding
    scope: Weak<ScopeInner>
    id: u64
    impl Drop：从任务作用域移除本项，不持有任务或World强引用。
ToolTaskHandle
    registration_scope(&self) -> TaskScope
    close_registrations(&self)
    same_task(&self, other: &Self) -> bool
    行为：cancel/complete_cleanup关闭作用域，Return不关闭；物理清理与撤销登记分开收口。


```

# error

## 类型

公开：
```text
ToolError：工具执行错误
    kind: ToolErrorKind
    message: String
    details: Option<serde_json::Value>--创建失败的phase及registered事实；网络边界筛选，不复制内部句柄
    impl Clone + Display + std::error::Error

ToolErrorKind：错误类别，公开枚举，Clone
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
│   ├── TaskUpdateContext → 通用变更回调
│   └── TaskSetShared → BTreeMap<task_id, ToolTaskHandle>
│       ├── 输出缓存、cursor、return状态与唯一结果回复端
│       ├── Lua输入通道
│       └── TaskScope → 精确注册撤销动作（Weak World）
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
