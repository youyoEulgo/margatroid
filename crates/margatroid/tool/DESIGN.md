# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System分别列出事件、闭包结果、提交闭包和oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
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
parse_tool_result(value: &serde_json::Value, limits: &ToolExecutionLimits) -> Result<ToolExecuteResponse, ToolError>
    行为：字符串原样作为文本，其他值序列化为content；识别ok/content/images结果信封；检查文本、图片及整体预算，不创建消息领域类型。
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
            require 执行设施可用、预算有效、未安装
            require schedule in [RuntimePlugin::UPDATE, RuntimePlugin::POST_UPDATE]
            install init_tool_tasks_system, tool_execute_system
```

# components

## 类型

公开：
```text
ToolTasks：该Agent的工具任务句柄集合，公开组件
    entries: Arc<Mutex<BTreeMap<String, ToolTaskHandle>>>--私有，宿主与任务共享的句柄集合
    register(&self, id: String, handle: ToolTaskHandle) -> Result<(), ToolError>
        require id未登记
        entries[id] = handle -- 空缓存，cursor=0，returned=false，唯一reply
    cancel(&self, id: &str) -> Result<(), ToolError>
        行为：向对应句柄请求取消，不直接中断监督任务或删除句柄。
    list(&self) -> Vec<(String, ToolTaskHandle)>
        return entries中的活任务句柄克隆
    read(&self, id: &str) -> Result<TaskReadResult, ToolError>
        行为：从集合取得仍存活任务后，同一短锁读取output[cursor..]并将相邻字符串无分隔符拼成一项，非字符串原样保留顺序；读取活任务error，将cursor推进至当前末尾，返回并清除replaced标记。
    send(&self, id: &str, value: serde_json::Value) -> Result<(), ToolError>
        行为：向目标Lua输入通道投递一个值；目标已finish返回TaskMissing；通道关闭或满返回错误，不宣称接收方已处理。
    finish(&self, id: &str) -> Future<Output = Result<(), ToolError>>
        request cancel(id)
        await Lua、进程及后代、管道、临时资源真实清理
        success: entries.remove(id); release cache; return Ok(())
        failure: return error; retain supervision
    abort_unreturned(&self) -> 工具中止交接结果
        for handle in entries:
            atomic if !handle.returned:
                reply 原call_id/alias的中止结果
                handle.returned = true
                handle.cancel() -- 同步触发，不等待清理
    impl Clone for ToolTasks
        行为：共享同一集合，不借用World。
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
    行为：InitToolTasks -> handle_init_tool_tasks。

tool_execute_system(world: &mut World)
    处理事件：ToolExecuteRequest
    接收闭包结果：Result<ToolLaunchOutput, ToolLaunchError>
    提交闭包：launch_tool(ToolExecutionContext) -> Future<Output = Result<ToolLaunchOutput, ToolLaunchError>>
    产出事件：None
    oneshot回复：Result<ToolExecuteResponse, ToolError>
    行为：在中止schedule之后执行；ToolExecuteRequest -> handle_tool_execute，逐请求复检当前turn，不使用发出时状态代替受理检查。
```

# handler

## 函数

crate公开：
```text
handle_init_tool_tasks(world: &mut World, request: InitToolTasks)
    行为：认领reply，检查agent实体；不存在ToolTasks则挂载空集合，已存在不覆盖；组件可查询后回复Ok(())，失败回复ToolError。

handle_tool_execute(world: &mut World, request: ToolExecuteRequest)
    行为：
        认领reply，核对Agent完整Entity、Working及当前非空turn与request.turn匹配；无turn或不匹配直接返回保留call_id/alias的中止工具结果，不注册任务、不启动闭包或进程。
        entry = Tools.get(request.alias) or return 对应工具错误
        使用entry.input_schema校验arguments
        按entry.path执行；owner_task_id与binding放入context
        从SelectedSandbox及Sandboxes取得可选策略路径
        -- 模型参数不能覆盖宿主context标记
        固定本次工具、策略路径和目录参数，准备owned执行材料，不跨await借用World。
        创建共享ToolTaskHandle，一份登记ToolTasks，克隆一份及共享集合句柄交给工作任务；通过send_async_closure提交launch_tool，失败通过原reply回复错误；不缓存执行器Handle。
```

私有：
```text
launch_tool(context: ToolExecutionContext) -> Future<Output = Result<ToolLaunchOutput, ToolLaunchError>>
    行为：在普通异步闭包内部创建本领域监督任务，立即把真实JoinHandle交给已登记的ToolTaskHandle，再返回启动结果。该闭包结果只证明启动交接，不表示工具完成；supervise_tool才负责业务reply和真实清理。启动异常记录于原ToolTask并完成原reply，System只收口启动事实，不反向控制其他领域。

supervise_tool(context: ToolExecutionContext) -> Future<Output = ()>
    行为：
        blocking worker按ToolEntry.path读取要执行的程序，schema直接使用Tools记录；sandbox_resource_core::read_sandbox读取策略，不重新读取工具定义。
        校验arguments与本入口的有效schema、具体后端策略；检查取消及所有者存活后启动run_runner，动态入口不读取注册schema作回退。配置及execute收到相同可信owner_task_id/binding/resource_dir，不接受arguments覆盖。
        接收runner的Return时通过共享句柄的原子return裁决交付结果，不结束runner；中止、错误与正常Return共用唯一回复端，不能在context另持独立可回复状态；持续处理缓存写入、覆盖及任务输入。
        自然退出/finish/到期/中止:
            World闭包: Tools.remove_owner(task_id)
            for B in ToolTasks where B.owner_task_id == task_id and !B.returned:
                原子回复B对应工具错误; request B.cancel
            未return则交付对应失败结果
            await VM、runner、后代、管道、临时文件真实清理
            ToolTasks移除本任务; release cache
        已回复调用不得重复回复；后台异常记录于对应任务error，未回复调用按实际错误完成。
        清理失败及时交付CleanupFailed；保持未finish并保留本次runner管理能力继续清理，不标记已finish。

run_runner(context: RunnerContext) -> Future<Output = Result<(), ToolError>>
    行为：启动可信supervisor及受限worker；并行投递stdin、读取有界stdout/stderr和等待退出；取消或超时终止执行并等待清理；处理双向协议帧；Return不是进程终态，真实退出清理后返回。

run_tool_worker(request: RunnerRequest) -> Result<(), ToolError>
    行为：在受沙箱限制的worker内创建Lua，先以纯计算环境加载脚本并运行configure(arguments, context)，内存及指令预算始终生效；配置经Configured帧往返确认后安装IO/OS和任务宿主，再运行execute(arguments, context)，不重新加载脚本。自然退出也进入清理；task_finish自身以控制退出终止后续Lua语句，不等待自身JoinHandle。
```

## Lua宿主函数

```text
task_write(value)
    行为：向当前任务缓存追加一个完整结构化值，不移动cursor；超预算明确报错，不静默丢未读内容。
task_replace(value)
    行为：将缓存整体替换为[value]并置cursor=0、replaced=true；覆盖与读取使用同一短锁。
task_return(value)
    行为：校验并转换为ToolExecuteResponse，原子竞争共享return状态及reply并交付一次；不释放VM、不关闭任务输入。中止已强制return后不得再次回复。
task_finish(task_id)
    行为：仅操作当前Agent任务；自身通过控制退出结束Lua，由监督者完成清理和集合移除；其他任务调用在真实清理并移除后才成功。不能先声明finished再清理；已finish目标不存在。
task_read(task_id)
    行为：读取同Agent活任务缓存的新增值和错误，并推进cursor；不返回completed状态，已finish目标返回TaskMissing。
task_send(task_id, value)
    行为：向同Agent目标任务的Lua输入通道投递任意结构化值。
task_receive()
    行为：异步接收当前Lua的一条输入，不持锁等待；输入含义及转发位置由工具代码决定。动态调用等待同时受B自身Task生命周期与所有者依赖通知控制，A关闭时返回错误而非无限等待。

tool_inject(definitions: Vec<InjectedToolDefinition>) -> 注册入口结果
    require configure已确认
    通过既有worker帧提交definitions
    宿主填path=当前工具资源，owner_task_id=当前任务
    World闭包:
        require 当前任务仍存在且未取消
        validate definitions
        Tools.insert_batch(definitions) -- alias冲突整批报错
    return 登记结果 -- 清理出口移除本任务所属记录

process_open(options) -> ProcessSession
    options.program: String--明确的可执行文件路径
    options.arguments: Vec<String>--argv数组，禁止隐式shell拼接
    在当前沙箱worker内spawn(program, arguments, cwd=context.project_root)
    stdin/stdout/stderr = pipes
    return ProcessSession -- 子进程归当前ToolTask
    -- 沿用ToolTask的timeout_ms、取消及finish清理

ProcessSession：Lua userdata，实际进程和管道仅由worker持有，不作为JSON帧传回宿主
    write(bytes: LuaString) -> Future<Output = ()>
        await 按写入顺序将完整bytes写入stdin -- 保持原始字节、支持背压
    read() -> Future<Output = {data: LuaString, eof: bool}>
        return await stdout有界字节块及EOF -- 单一读取者；消息边界由工具解析
    read_stderr() -> Future<Output = {data: LuaString, eof: bool}>
        return await stderr有界字节块及EOF -- 独立消费，超预算报错
    wait() -> Future<Output = {exit_code: Option<i32>, signal: Option<i32>}>
        await 进程退出并回收
        return exit_code, signal
    close() -> Future<Output = ()>
        await 终止并回收子进程及管道 -- 不自动finish当前ToolTask
    -- 错误沿ToolError交付
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
    InputConsumed--Lua取走输入后允许转发下一条；宿主响应读取不等待输入邮箱腾位。

HostFrame：supervisor到worker的协议帧，私有枚举
    Input(serde_json::Value)
    Response { request_id: String, result: Result<serde_json::Value, ToolError> }


ToolContext：worker上下文
    task_id: String
    call_id: String
    alias: String--本次被调用的入口alias，不一定是原注册入口
    owner_task_id: Option<String>--宿主设置；注册入口None，动态入口为注入者A，不接受模型填写
    binding: Option<serde_json::Value>--脚本自定义不透明数据，宿主保存并原样注入
    resource_dir: PathBuf--当前工具资源目录，供定位同目录bin；不从arguments派生
    project_root: PathBuf
    temp_dir: PathBuf
```

# types pub

## 类型

公开：
```text
ToolTaskHandle：可克隆的任务共享句柄，公开类型
    inner: Arc<ToolTaskShared>
    is_cleanup_complete(&self) -> bool--仅内部清理等待者使用，不是对外completed状态或完成缓存
    impl Clone
        行为：共享同一任务状态、输入与取消能力，不复制任务。

ToolTaskShared：任务持有数据
    cancel: CancellationToken--存活任务取消能力；回合中止只触发原子裁决仍未return的任务，不取消已return后台任务
    task: Mutex<Option<JoinHandle<()>>>--监督任务句柄，取消时不直接abort
    shared: Mutex<ToolTaskData>--returned状态与唯一reply在同一锁内裁决
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
    owner_task_id: Option<String>--动态调用所属A，供A退出时找到仍等待它的B，不另建依赖注册表

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

ToolExecutionLimits：单次执行预算
    script_bytes: usize
    argument_bytes: usize
    output_bytes: usize
    result_bytes: usize
    diagnostic_bytes: usize
    memory_bytes: usize
    instruction_budget: u64
    cache_bytes: usize
    input_capacity: usize

ToolConfiguration：Lua配置函数返回的本次时间预算
    timeout_ms: u64
    行为：仅此一个必填字段，正整数为ToolTask存活毫秒数，0不限时；未知字段、负数及不可表示时间拒绝，无后端默认值。ToolTask保存配置确认时确定的deadline，到期取消并等待真实清理后完成，不另设宿主、shell或清理限时。只有configure确认后才安装执行能力并调用execute。
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
    TaskPanicked | CleanupFailed | AlreadyReturned | MissingReturn | InputClosed | InputFull
    OwnerGone--动态入口所有者已不接受调用，依赖调用及时失败，不识别远端协议
```

# 持有关系

```text
Agent Entity
├── Agent（agent_core）
├── Tools（tool_resource_core）
├── ToolTasks（tool_core）
│   └── BTreeMap<task_id, ToolTaskHandle>
│       ├── 输出缓存、cursor、return状态与唯一结果回复端
│       ├── Lua输入通道
│       └── 动态入口所有者/依赖关系（不理解MCP）
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
