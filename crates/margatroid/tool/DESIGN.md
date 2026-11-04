# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
System分别列出事件、闭包结果、提交闭包和oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 设计范围

Margatroid领域使用原生Axum装配及普通异步闭包，已退出mecs通用网络桥和spawn_async_service依赖。mecs现有接口、文档及源码暂时保留，后续另行清理，不提供旧入口回退或兼容实现。

# 板块格式

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
            行为：检查执行设施和预算，拒绝重复安装；安装init_tool_tasks_system和tool_execute_system。
```

# components

## 类型

公开：
```text
ToolTasks：该Agent的工具任务句柄集合，公开组件
    entries: Arc<Mutex<BTreeMap<String, ToolTaskHandle>>>--私有，宿主与任务共享的句柄集合
    register(&self, id: String, handle: ToolTaskHandle) -> Result<(), ToolError>
        行为：登记本次任务及空缓存、cursor=0、completed=false，重复id返回错误，不覆盖已有条目。
    cancel(&self, id: &str) -> Result<(), ToolError>
        行为：向对应句柄请求取消，不直接中断监督任务或删除句柄。
    list(&self) -> Vec<(String, ToolTaskHandle)>
        行为：克隆工具任务句柄列表，不包括推理、存储或VM。
    read(&self, id: &str) -> Result<TaskReadResult, ToolError>
        行为：同一短锁读取output[cursor..]并将相邻字符串无分隔符拼成一项，非字符串原样保留顺序；读取completed及error，将cursor推进至当前末尾，返回并清除replaced标记。
    send(&self, id: &str, value: serde_json::Value) -> Result<(), ToolError>
        行为：向目标Lua输入通道投递一个值；任务已结束或通道满返回错误，不宣称接收方已处理。
    finish(&self, id: &str) -> Future<Output = Result<(), ToolError>>
        行为：请求目标退出并等待监督任务清理；保留任务条目和缓存。
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
    行为：ToolExecuteRequest -> handle_tool_execute。
```

# handler

## 函数

crate公开：
```text
handle_init_tool_tasks(world: &mut World, request: InitToolTasks)
    行为：认领reply，检查agent实体；不存在ToolTasks则挂载空集合，已存在不覆盖；组件可查询后回复Ok(())，失败回复ToolError。

handle_tool_execute(world: &mut World, request: ToolExecuteRequest)
    行为：
        认领reply，核对Agent完整Entity及是否允许新工具调用，不检查turn。
        从Tools取得alias路径，从SelectedSandbox及Sandboxes取得可选策略路径。
        固定本次工具、策略路径和目录参数，准备owned执行材料，不跨await借用World。
        创建共享ToolTaskHandle，一份登记ToolTasks，克隆一份及共享集合句柄交给工作任务；通过send_async_closure提交launch_tool，失败通过原reply回复错误；不使用spawn_async_service，不缓存执行器Handle。
```

私有：
```text
launch_tool(context: ToolExecutionContext) -> Future<Output = Result<ToolLaunchOutput, ToolLaunchError>>
    行为：在普通异步闭包内部创建本领域监督任务，立即把真实JoinHandle交给已登记的ToolTaskHandle，再返回启动结果。该闭包结果只证明启动交接，不表示工具完成；supervise_tool才负责业务reply和真实清理。启动异常记录于原ToolTask并完成原reply，System只收口启动事实，不反向控制其他领域。

supervise_tool(context: ToolExecutionContext) -> Future<Output = ()>
    行为：
        blocking worker调用tool_resource_core::read_tool和sandbox_resource_core::read_sandbox，读取本次程序、schema和策略。
        校验arguments与本次schema、具体后端策略；检查取消后启动run_runner。
        接收runner的Return时完成原reply一次，不结束runner；持续处理缓存写入、覆盖及跨任务读取输入请求。
        等待VM、runner、后代、管道和临时文件真实清理后设置completed=true并保留条目；若尚未回复则返回未回复结束错误。
        已回复调用不得重复回复；后台异常记录于对应任务error，未回复调用按实际错误完成。
        清理失败返回CleanupFailed并保留本次runner管理能力，不假称完成。

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
    行为：校验并转换为ToolExecuteResponse，通过原调用reply回复一次；不释放VM、不关闭任务输入。
task_finish(task_id)
    行为：仅操作当前Agent任务；自身为控制退出，其他任务请求结束并等待真实清理；完成前不得置completed=true。
task_read(task_id)
    行为：读取同Agent目标缓存的新增值、完成标记和错误，并推进cursor。
task_send(task_id, value)
    行为：向同Agent目标任务的Lua输入通道投递任意结构化值。
task_receive()
    行为：异步接收当前Lua的一条输入，不持锁等待；输入含义及转发位置由工具代码决定。
```

# types

## 类型

私有：
```text
ToolLaunchOutput：监督任务已交给ToolTaskHandle的启动事实，不是工具执行结果
ToolLaunchError：ToolError及原请求回复/共享句柄关联，实现From<AsyncTaskError>

ToolExecutionContext：单次执行材料
    request: ToolExecuteRequest
    reply: oneshot::Sender<Result<ToolExecuteResponse, ToolError>>
    tool_path: PathBuf
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
    InputConsumed--Lua取走输入后允许转发下一条；宿主响应读取不等待输入邮箱腾位。

HostFrame：supervisor到worker的协议帧，私有枚举
    Input(serde_json::Value)
    Response { request_id: String, result: Result<serde_json::Value, ToolError> }


ToolContext：worker上下文
    task_id: String
    call_id: String
    alias: String
    project_root: PathBuf
    temp_dir: PathBuf
```

# types pub

## 类型

公开：
```text
ToolTaskHandle：可克隆的任务共享句柄，公开类型
    inner: Arc<ToolTaskShared>
    impl Clone
        行为：共享同一任务状态、输入与取消能力，不复制任务。

ToolTaskShared：任务持有数据
    cancel: CancellationToken--任务独立取消信号，与turn及原reply生命周期无关
    task: Mutex<Option<JoinHandle<()>>>--监督任务句柄，取消时不直接abort
    shared: Mutex<ToolTaskData>
    input: mpsc::Sender<serde_json::Value>

ToolTaskData：任务查询数据
    output: Vec<serde_json::Value>
    cursor: usize
    replaced: bool--上次查询后是否发生整体替换，初始false
    completed: bool
    error: Option<ToolError>

TaskReadResult：增量查询结果
    replaced: bool--true表示本次内容替换此前展示，false表示追加
    output: Vec<serde_json::Value>
    completed: bool
    error: Option<ToolError>

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
```

# 持有关系

```text
Agent Entity
├── Agent（agent_core）
├── Tools（tool_resource_core）
├── ToolTasks（tool_core）
│   └── BTreeMap<task_id, ToolTaskHandle>
│       ├── 输出缓存、cursor与完成状态
│       └── Lua输入通道
└── Sandboxes与SelectedSandbox（sandbox_resource_core）

mecs全局异步设施
└── supervise_tool
    ├── ToolExecutionContext
    │   └── 原reply
    ├── 本次工具及策略材料
    └── runner监督能力
        ├── 可信supervisor
        ├── 受限worker及后代
        └── 管道和临时文件
```
