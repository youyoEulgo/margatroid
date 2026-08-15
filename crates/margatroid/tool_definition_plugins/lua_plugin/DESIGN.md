# LuaPlugin

## 类型

公开：
```text
LuaPlugin：Lua工具定义与执行Plugin，公开结构体--注册type=tool资源并异步执行可信Lua脚本
    home_root: Arc<PathBuf>--主目录Tool根，例如~/.margatroid/tools
    limits: LuaExecutionLimits--每次Lua调用的固定安全限制
    open(home_root: impl Into<PathBuf>) -> Result<Self, LuaError>
        打开Plugin：公开关联函数，规范化绝对主目录并使用有界默认LuaExecutionLimits
    with_limits(self, limits: LuaExecutionLimits) -> Result<Self, LuaError>
        设置限制：公开构建方法，验证全部限制非零后替换默认值
    impl Plugin for LuaPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建Plugin：要求RuntimePlugin、AsyncRuntimePlugin和ToolPlugin已安装
            行为：
                重复安装时panic
                插入LuaRoots、LuaExecutionLimits和具体直接能力所需的共享客户端
                在RuntimePlugin::UPDATE挂载lua_tool_register_system、lua_tool_call_prepare_system和lua_task_result_system
                使用AppAsyncExt::add_async_system挂载execute_prepared_lua_tool

LuaExecutionLimits：Lua单次执行限制，公开结构体--所有字段在调用开始前固化且Lua不可读取或修改
    max_definition_bytes: usize--tool.toml和input.schema.json分别允许的最大字节数
    max_script_bytes: usize--main.lua最大字节数
    max_argument_bytes: usize--ToolCallRequest.arguments最大字节数
    max_output_bytes: usize--成功结果最大UTF-8字节数
    max_memory_bytes: usize--Lua VM最大内存
    max_instructions: u64--单次执行最大Lua指令数
    max_execution_time: Duration--包括全部宿主调用的总执行时间
    max_host_call_time: Duration--单次异步宿主调用最大时间
    new(...) -> Result<Self, LuaError>
        构造限制：公开关联函数，要求全部数值非零且单次宿主调用时间不超过总执行时间
    impl Default for LuaExecutionLimits
        Default：公开trait实现
        行为：max_definition_bytes=64KiB、max_script_bytes=4MiB、max_argument_bytes=1MiB、max_output_bytes=16MiB、max_memory_bytes=256MiB、max_instructions=100000000、max_execution_time=15分钟、max_host_call_time=5分钟
    impl Resource for LuaExecutionLimits
        Resource：公开trait实现，LuaPlugin安装后作为World只读配置

LuaToolRegisterRequest：Agent Lua工具注册请求，公开事件
    id: String--Workspace注册子请求ID
    agent: Entity--目标Agent Entity，必须已挂载AgentIdentity、AgentToolMap和AgentToolEnvironment
    resource_id: ResourceId--待注册完整Tool资源ID
    impl Event for LuaToolRegisterRequest

LuaToolRegisterResponse：Agent Lua工具注册结果，公开事件
    id: String--原注册子请求ID
    agent: Entity
    resource_id: ResourceId
    result: Result<(), ToolError>
    impl Event for LuaToolRegisterResponse

LuaError：LuaPlugin配置错误，公开结构体--只描述Plugin构造与依赖错误，不回显脚本、参数、绝对路径或响应正文
LuaErrorKind：LuaPlugin配置错误分类，公开枚举
    InvalidRoot
    InvalidLimits
    DependencyMissing
    AlreadyInstalled
```

私有：
```text
LuaRoots：Lua工具主目录配置，私有Resource
    home_root: Arc<PathBuf>
    impl Resource for LuaRoots

LuaToolMetadata：tool.toml静态元信息，私有结构体--反序列化时拒绝未知字段
    schema_version: u32--当前只允许1
    name: String--必须与resource_id.name相同且非空
    description: String--非空模型可见说明

LuaToolDefinition：已验证Lua工具定义，私有结构体
    metadata: LuaToolMetadata
    parameters: serde_json::Value--input.schema.json的JSON Schema对象

LuaToolPackage：一次执行重新读取的完整工具包，私有结构体
    definition: LuaToolDefinition
    script: String--main.lua UTF-8正文

LuaToolCallLocator：ToolCallResponse定位信息，私有结构体
    turn_id: String
    agent: Entity
    tool_call_id: String

LuaCallContext：注入Lua的只读调用信息，私有结构体
    agent_id: ResourceId--来自AgentIdentity，不向Lua暴露Entity
    turn_id: String
    resource_id: ResourceId
    project_root: Arc<PathBuf>--公开给可信Lua工具的项目绝对根
    image_root: Arc<PathBuf>--公开给可信Lua工具的镜像绝对根
    package_root: Arc<PathBuf>--公开给可信Lua工具的当前工具包绝对根

LuaExecutionHandle：单次Lua执行能力集合，私有结构体--不实现Component或Resource
    context: LuaCallContext
    capabilities: LuaDirectCapabilityHandle
    limits: LuaExecutionLimits

LuaDirectCapabilityHandle：单次调用直接能力集合，私有结构体
    fs: LuaFileHandle
    http: LuaHttpHandle
    json: LuaJsonHandle
    log: LuaLogHandle

LuaFileHandle：开放文件便利能力，私有结构体--接受任意绝对或相对路径，不持有World
    limits: LuaExecutionLimits
    install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError>
        注入文件API：私有方法，创建margatroid.fs
        行为：
            read_text(path: String) -> async String--异步读取UTF-8文本
            write_text(path: String, content: String) -> async ()--异步覆盖写入UTF-8文本，不自动创建父目录
            create_dir_all(path: String) -> async ()--异步递归创建目录
            remove(path: String) -> async ()--文件使用remove_file，目录使用remove_dir_all
            rename(from: String, to: String) -> async ()--异步重命名
            list(path: String) -> async Vec<{ name: String, path: String, kind: "file" | "directory" | "symlink" | "other" }>--异步读取一层目录并按name排序
            所有路径直接交给操作系统；相对路径相对于daemon进程工作目录，不做根目录限制或符号链接检查
            单次读写正文受max_output_bytes限制，单次调用受max_host_call_time限制

LuaHttpHandle：开放HTTP便利能力，私有结构体--持有可克隆HTTP客户端和限制，不持有World
    client: reqwest::Client
    limits: LuaExecutionLimits
    install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError>
        注入HTTP API：私有方法，创建margatroid.http
        行为：
            request(options: Table) -> async { status: u16, headers: Map<String, String>, body: String }
            options.method为可选String且默认GET；options.url为必填String；options.headers为可选String到String表；options.body为可选String
            使用reqwest默认重定向、系统代理和Plugin构造的TLS配置；不限制协议目标、主机、端口或本地网络
            单次调用受max_host_call_time限制，响应body超过max_output_bytes时失败

LuaJsonHandle：JSON能力，私有单元结构体--不持有外部状态
    install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError>
        注入JSON API：私有方法，创建margatroid.json
        行为：
            encode(value: LuaValue) -> String--使用mlua serde转换并序列化紧凑JSON
            decode(source: String) -> LuaValue--解析JSON并使用mlua serde转换为Lua值
            NaN和无穷浮点数按serde_json规则拒绝；Lua table不能无损表示时返回Lua错误

LuaLogHandle：结构化日志能力，私有结构体--固定本次调用日志上下文，不持有World
    agent_id: ResourceId
    turn_id: String
    resource_id: ResourceId
    install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError>
        注入日志API：私有方法，创建margatroid.log
        行为：注入trace(message)、debug(message)、info(message)、warn(message)和error(message)同步函数；message是String并携带固定agent_id、turn_id和resource_id tracing字段

LuaDomainCommandHandle：领域命令能力，预留私有结构体--首版不构造也不注入Lua
    边界：未来只能持有受限RuntimeEventSender和逐请求响应通道，不得持有World或直接修改ECS状态

LuaToolResponseGuard：异步工具回执守卫，私有结构体--从提交异步请求前开始保证恰好一次ToolCallResponse
    locator: Option<LuaToolCallLocator>--Some表示尚未回执，成功回执后take为None
    events: RuntimeEventSender--可跨线程发送ToolCallResponse并唤醒Runtime
    new(request: &ToolCallRequest, events: RuntimeEventSender) -> Self
        构造守卫：私有关联函数，克隆请求定位信息
    respond(&mut self, result: Result<String, ToolError>)
        完成回执：私有方法，take locator并发送ToolCallResponse；locator已为空时panic
    impl Drop for LuaToolResponseGuard
        Drop：私有trait实现
        drop(&mut self)
            兜底回执：locator仍为Some时发送ExecutionFailed的ToolCallResponse并清空locator

PreparedLuaToolCall：准备完成的Lua工具调用，私有事件--只携带Send且不借用World的数据
    package_root: Arc<PathBuf>
    arguments: String--原始JSON对象文本
    handle: LuaExecutionHandle
    response: LuaToolResponseGuard--提交异步请求前已经创建
    impl Event for PreparedLuaToolCall

LuaTaskError：异步基础设施错误，私有结构体--只用于记录AsyncRuntime结果，工具回执由LuaToolResponseGuard负责
    source: AsyncTaskError
    impl From<AsyncTaskError> for LuaTaskError
```

## 函数

私有：
```text
lua_tool_register_system(world: &mut World)
    注册Lua工具：私有System，读取LuaToolRegisterRequest
    行为：
        验证id非空、agent存活且resource_id使用type=tool
        从Agent Entity读取AgentToolEnvironment
        按项目、镜像、主目录顺序精确查找工具包
        有界读取tool.toml和input.schema.json并调用parse_lua_tool_definition
        构造ToolTemplate { name=resource_id.to_string(), description, parameters }；AgentToolMap注册时再替换为Agent内tool_name
        调用register_agent_tool，tool_id固定为tool:builtin/lua-runtime:latest，resource_id保持具体Tool ID
        成功或失败都发送LuaToolRegisterResponse
        不读取或执行main.lua，不测试工具副作用

lua_tool_call_prepare_system(world: &mut World)
    准备Lua调用：私有System，读取ToolCallRequest
    行为：
        只处理tool_id=tool:builtin/lua-runtime:latest
        为每个请求先验证定位字段、agent、type=tool和arguments字节限制
        从Agent读取AgentIdentity和AgentToolEnvironment；不读取Workspace组件
        按项目、镜像、主目录顺序重新查找工具包
        构造LuaCallContext { agent_id, turn_id, resource_id, project_root, image_root, package_root }
        使用AgentToolEnvironment、工具包根、共享客户端和LuaExecutionLimits构造LuaExecutionHandle
        在send_async_event前使用world.event_sender构造LuaToolResponseGuard
        成功时发送PreparedLuaToolCall异步事件
        任一步失败时立即发送且只发送一次ToolCallResponse { result: Err(error), 原定位字段 }

execute_prepared_lua_tool(prepared: PreparedLuaToolCall) -> Result<(), LuaTaskError>
    执行Lua工具：私有async system
    行为：
        有界异步读取tool.toml、input.schema.json和main.lua，构造LuaToolPackage
        解析arguments为JSON对象并按当前input.schema.json校验
        创建独立Lua 5.4 VM并应用内存限制、指令Hook和总截止时间
        使用mlua unsafe构造方式加载完整Lua标准库，包括io、os、package、debug和动态模块能力
        创建只读context table，包含agent_id、turn_id、resource_id、project_root、image_root和package_root字符串
        创建margatroid table并分别调用四个直接能力Handle.install
        加载main.lua，要求全局execute是函数
        使用mlua::Function::call_async调用execute(arguments, context)
        异步宿主函数等待期间挂起Lua协程，不阻塞Runtime tick
        要求结果是UTF-8字符串且不超过max_output_bytes
        将成功或稳定ToolError交给prepared.response.respond
        所有脚本、参数、Schema、宿主调用和超时错误都转换为ToolError，不作为LuaTaskError返回
        正常路径返回Ok(())；panic或任务取消由AsyncRuntime转换为LuaTaskError，response Drop发送兜底回执

lua_task_result_system(world: &mut World)
    收集异步基础设施结果：私有System，读取Result<(), LuaTaskError>
    行为：Ok不处理；Err只记录不含脚本、参数和绝对路径的稳定警告，不再发送ToolCallResponse

find_lua_tool_package(environment: &AgentToolEnvironment, home_root: &Path, resource_id: &ResourceId) -> Result<PathBuf, ToolError>
    查找Lua工具包：私有函数
    行为：
        只接受type=tool的完整ResourceId，tag不限定为latest
        依次查询<project>/.margatroid/tools、<image>/tools和home_root
        每个根依次拼接scope、name和tag
        不存在时尝试下一根；找到非目录、目录不可检查或缺少任一必需文件时立即失败且不回退
        要求tool.toml、input.schema.json和main.lua都是普通文件
        返回工具包目录，不返回脚本正文

parse_lua_tool_definition(metadata_source: &str, schema_source: &str, resource_id: &ResourceId) -> Result<LuaToolDefinition, ToolError>
    解析Lua工具定义：私有函数
    行为：
        使用TOML严格解析LuaToolMetadata并拒绝未知字段
        要求schema_version=1、name非空且等于resource_id.name、description非空
        将input.schema.json解析为JSON对象并验证为可接受的模型工具参数Schema
        不执行Lua，不改写ToolTemplate名称

read_lua_tool_package(package_root: &Path, resource_id: &ResourceId, limits: &LuaExecutionLimits) -> Result<LuaToolPackage, ToolError>
    读取执行工具包：私有async函数
    行为：分别有界读取三个文件，复用parse_lua_tool_definition并要求main.lua是非空UTF-8文本

install_lua_environment(lua: &Lua, handle: &LuaExecutionHandle) -> Result<(), ToolError>
    安装Lua环境：私有函数
    行为：创建只读context与margatroid table，依次安装开放fs、http、json和log便利能力；不暴露LuaExecutionHandle本体
```

## 逻辑

```text
安装：
    RuntimePlugin
        -> AsyncRuntimePlugin
        -> ToolPlugin
        -> LuaPlugin::open(<data_root>/tools)

Workspace启动：
    BuiltinToolPlugin
        -> 接收Workspace提交的tool资源
        -> 发送LuaToolRegisterRequest
    LuaPlugin
        -> 读取静态定义
        -> register_agent_tool(
               agent,
               tool:builtin/lua-runtime:latest,
               具体tool资源ID,
               ToolTemplate
           )
        -> LuaToolRegisterResponse
    WorkspacePlugin
        -> 所有资源注册成功后Workspace才进入ready

每次调用：
    ToolPlugin
        -> PendingToolCalls.add_pending
        -> ToolCallRequest
    LuaPlugin同步准备System
        -> 创建LuaToolResponseGuard
        -> send_async_event(PreparedLuaToolCall)
    AsyncRuntimePlugin
        -> execute_prepared_lua_tool
        -> Lua同步计算
        -> 异步Rust宿主函数通过Lua协程等待
        -> ToolCallResponse
    ToolPlugin
        -> PendingToolCalls.remove
        -> AgentMessage::Tool
        -> ToolTurnCompleted
```

## 边界

```text
LuaPlugin依赖AgentPlugin的AgentIdentity、ToolPlugin和AsyncRuntimePlugin；不依赖WorkspacePlugin，避免与注册协调形成循环依赖。
LuaPlugin不读取Agent可见性；BuiltinToolPlugin只把已经确定的具体resource_id交给它注册。
LuaPlugin不持有AgentToolMap、PendingToolCalls或AgentStatus，不构造AgentMessage，不判断一轮工具是否完成。
LuaExecutionHandle及全部子句柄不持有World、Entity查询器、Component引用或Resource引用。
Agent Entity只用于内部ToolCallResponse定位；Lua可见上下文使用AgentIdentity中的ResourceId。
Lua工具是开发者主动安装的可信代码；LuaPlugin不构成安全沙箱，完整标准库和开放句柄允许任意文件、环境、网络、子进程和动态模块操作。
LuaExecutionLimits只防止部分意外资源耗尽，不是权限边界；os.execute、原生模块和其他同步系统调用可能绕过异步超时与Lua指令Hook。
首版LuaDomainCommandHandle不存在；工具互调、Agent、Skill、Workspace和Inference不能由Lua直接操作。
Lua脚本每次调用重新读取并使用新VM；不同调用不共享Lua全局变量或运行时模块缓存。
异步只允许宿主Future让出执行；纯Lua计算仍同步，必须由指令Hook和截止时间中止。
```
