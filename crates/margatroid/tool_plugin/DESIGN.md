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

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        Plugin 与 crate 公开函数
# system     src/system.rs     System 函数
# handler    src/handler/      内置工具 handler 目录
# events     src/events.rs     事件类型
# types      src/types.rs      其余类型
# error      src/error.rs      Error 类型
```

tool_plugin 的 handler 是目录，包含基础回执处理和四类内置工具实现：

```text
src/handler/
├── mod.rs           finish_tool_call 与模块声明
├── skill.rs         skill 资源注册与执行
├── hook.rs          hook 资源注册与 no-op 执行
├── lua.rs           lua 工具注册、准备与异步执行
├── pty.rs           命令的终端执行：分配 PTY、回收进程组、超时与输出限额
├── sandbox.rs       沙箱策略解析、受限 spawn 与进程组
```

# lib

## 类型

公开：
```text
ToolPlugin：工具插件，公开结构体--统一安装内置工具根、注册路由和调用路由
    schedule: String--System 所属 Schedule，私有
    skill_root: Arc<PathBuf>--skill 根目录，私有
    hook_root: Arc<PathBuf>--hook 根目录，私有
    lua_root: Arc<PathBuf>--lua 工具根目录，私有
    lua_limits: LuaExecutionLimits--Lua 执行限制，公开（runner bin 目标要按同一份限额构造请求）
    open(root: impl Into<PathBuf>) -> Result<Self, ToolError>
        打开插件：公开关联函数，要求 root 绝对且无父级跳转
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置 Schedule：公开构建方法
    impl Default for ToolPlugin
    impl Plugin for ToolPlugin
        build(self, app: &mut App)
            安装插件：要求 RuntimePlugin 和 AsyncRuntimePlugin 已安装
            插入 ToolPluginInstalled、三类 roots 与 lua 限额
            依次挂载注册、调用路由、异步执行、任务结果、取消和清理 System

ToolPluginInstalled：工具插件安装标记，公开单元 Resource
    impl Resource for ToolPluginInstalled
```

## 函数

公开：
```text
register_agent_resource(world: &mut World, agent: Entity, entry: ResourceMapEntry) -> Result<ResourceMapEntry, ToolError>
    注册 Agent 资源：公开函数，把可执行条目写入 Agent.resources

candidate_resource_entry(resource_id: ResourceId, alias: Option<String>, tool_id: ResourceId, template: ToolTemplate) -> Result<ResourceMapEntry, ToolError>
    构造候选资源条目：公开函数

resolve_agent_tool_definitions(world: &World, agent: Entity, resources: &[ResourceId]) -> Result<Vec<ToolDefinition>, ToolError>
    解析 Agent 工具定义：公开函数，要求每个可见资源恰好注册一次

validate_agent_tool_calls(world: &World, agent: Entity, calls: &[ToolCall]) -> Result<(), ToolError>
    校验 Agent 工具调用：公开函数，校验 tool_name 已注册
```

# system

## 函数

crate公开：
```text
tool_register_system(world: &mut World)
    注册路由 System
    处理事件：ToolRegisterRequest
    行为：id 为空 -> InvalidRequest；skill/hook/sandbox/tool/prompt 类型交给对应 handler；未知类型 -> ProviderMissing

tool_call_route_system(world: &mut World)
    调用路由 System
    处理事件：ToolCallEvent
    行为：校验事件和参数，解析 ToolCallRequest，写入 Agent.tools.pending，按 executor 分派 skill/hook/lua handler
    说明：命令统一经 Lua 工具的 margatroid().shell 执行，shell 不是资源也不是工具

cancel_tool_turn_system(world: &mut World)
    取消工具轮次 System
    处理事件：CancelToolTurn
    行为：保留不属于该轮次的 pending 条目
```

# handler

```text
mod.rs   finish_tool_call(world, request, result)
            从 Agent.tools.pending 移除请求并发送 AgentMessage::Tool

skill.rs skill_register_system / execute_skill_call / parse_skill_document
hook.rs  hook_register_system / execute_hook_call
lua.rs   lua_tool_register_system / prepare_lua_call / execute_prepared_lua_tool / lua_task_result_system
         / run_lua_tool / spawn_tool_runner / runner_failure / tool_runner_path
prompt.rs
         prompt_register_system / register_prompt_resource / read_prompt_metadata
         / read_prompt_content / find_prompt_package
sandbox.rs
         sandbox_register_system / register_sandbox_resource / find_sandbox_package
         / active_sandbox_policies / sandbox_backend / ConfinedSpawn / CallTempDir / ProcessGroup

prompt_register_system(world)
    注册提示资源：处理 resource_id 类型为 prompt 的 ToolRegisterRequest
    行为：定位提示包 → 读取 prompt.toml 取 role → 读取 content.md 取正文
          → 校验 role 为 system、user 或 assistant，正文非空
          → 返回 ResourceMapEntry，content 为 ResourceContent::Prompt 且 executor 与 template 为空

find_prompt_package(library_root, resource_id)
    定位提示包：私有函数，按 <library_root>/prompts/<scope>/<name>/<tag> 定位
    行为：要求类型为 prompt 且 tag 为 latest；目录不存在则报资源未找到
    说明：作用域是普通作用域，与其它资源一致，不再是 system 或 user 这类角色名
        角色由 prompt.toml 的 role 字段描述，不编码进资源 ID

资源库根：
    handler::library_root(world) 从 ResourceLibraryRoot 取全局根目录并返回
    所有 find_* 都只在该根目录下的对应集合中查找
    不存在项目内 .margatroid 或镜像目录的回退路径，因此同名资源在全进程只有一份

执行时经图书馆读取，不自行拼接路径：
    描述与输入 schema 取自资源实体上的组件，导入时已校验
    脚本正文经 resource_library_plugin::read_tool_script 读取
    技能正文经 resource_library_plugin::read_skill_content 读取
    调用方不拼 library_root，读盘职责收在图书馆的 access 板块
    一次已经开始的调用持有自己的快照，重新导入不会改变它的内容

执行路径不再解析 TOML：
    tool.toml 只在导入时解析一次，用途是校验 schema_version、name 与描述
    校验通过后组件里留下的是解析好的描述与 schema，执行时直接使用
    LuaToolPackage 装的是 description: String 与 input_schema: Value
        而不再是两个语义不明的 String（其中一个曾被误当作 TOML 文本）
    这个错误因此在类型上不再可能发生

sandbox_register_system(world)
    注册沙箱资源：处理 resource_id 类型为 sandbox 的 ToolRegisterRequest
    行为：定位策略包 → 读取 policy.json → 校验非空、不超过 MAX_POLICY_BYTES、是合法 JSON
          → 返回 ResourceMapEntry，content 为 ResourceContent::Sandbox 且 executor 与 template 为空
    边界：失败只记日志并回错误响应，不影响其他资源

find_sandbox_package(library_root, resource_id)
    定位策略包：私有函数，只在全局资源库的 sandboxes 集合下查找
    行为：要求类型为 sandbox；命中含 policy.json 的目录即返回，否则报资源未找到
    说明：不存在项目内或镜像内的回退路径，资源全局唯一

active_sandbox_policies(world: &World, resources: &AgentResourceMap) -> Result<Vec<Arc<str>>, ToolError>
    解析本次调用生效的策略：crate 内公开
    行为：按 active_sandboxes 的集合序遍历，逐个经图书馆读取策略文本
    行为：按声明顺序返回，交由 landstrip 合并
    边界：资源不可读、或策略为空，一律报错而不是静默降级
    说明：策略只存在于沙箱资源实体上；Agent 只记录哪些沙箱被激活，不再另存一份策略文本
        快照粒度是一次调用：本次调用开始时取到的策略在这一轮内不变

sandbox_backend() -> Option<PathBuf>
    定位后端：环境变量 MARGATROID_SANDBOX_BACKEND → PATH 里的 landstrip → 当前可执行文件同目录

ConfinedSpawn：一次受限调用的全部约定，crate 内公开结构体
    policies: Vec<PathBuf>--本次调用暂存的策略文件，私有
    new(policies: &[Arc<str>]) -> Result<Self, ToolError>
        暂存策略：crate 内公开关联函数，逐份写进独立临时文件，任一份失败时清掉已写的
    is_confined(&self) -> bool
        是否受限：crate 内公开方法，策略集合非空即为真；同时决定是否整树回收
    command(&self, program: &Path, project_root: &Path) -> Result<tokio::process::Command, ToolError>
        构造命令：crate 内公开方法
        行为：受限时包成 landstrip run -p <策略>... -- <program>，未受限时直接执行 program；
              两种情况统一设置工作目录为项目根、继承环境但剔除凭据形变量、kill_on_drop
        边界：有策略而后端不可用则 fail-closed，拒绝执行而不是绕过策略
    impl Drop for ConfinedSpawn
        删除本次调用暂存的所有策略文件

    说明：工具进程共用本类型，工作目录、环境、策略暂存与回收模式只有这一处定义；
          工作目录必须是项目根，因为可移植策略写的是 "."，否则会解析到 daemon 自己的目录；
          环境是继承而非清空：把 HOME、locale、代理都留给工具，只拦下 KEY/PASSWORD/SECRET/TOKEN
          这类凭据形名字，否则普通 CLI 无法正常工作
          曾出现过的整表清空只保留 PATH，结果是 $HOME 展开为空，普通 CLI 因此失常
          凭据形名字按子串、忽略大小写匹配，与参照实现的判定方式一致
          管道模式下另外固定 NO_COLOR=1、TERM=dumb、PAGER=cat、GIT_PAGER=cat：
              工具这里读到的是管道而不是终端，分页与颜色不应进入被捕获的输出
              pty 路径由自己设置终端类型，不走这一组

CallTempDir：一次调用的临时目录，crate 内公开结构体
    path: PathBuf--<项目根>/.margatroid/tmp/<序号>，私有
    create(project_root: &Path) -> Result<Self, ToolError>
        创建临时目录：crate 内公开关联函数，供工具存放 stderr 等中间产物
    path(&self) -> &Path
        读取路径：crate 内公开方法，注入到工具上下文的 temp_dir
    impl Drop for CallTempDir
        递归删除该目录

    说明：受限调用除工作区外只允许写这里，所以策略里的项目根就已覆盖，不需要放开共享的 /tmp

ProcessGroup：受限调用的进程树回收，crate 内公开结构体
    leader: Option<u32>--进程组首进程；未受限时为 None，私有
    confined(confined: bool, leader: u32) -> Self
        构造：crate 内公开关联函数，仅当本次受限时才记录首进程
    reclaim(&mut self)
        回收进程树：crate 内公开方法，对首进程所在进程组发送 SIGKILL；幂等，首次调用后失效
    impl Drop for ProcessGroup
        兜底回收

    说明：kill_on_drop 只覆盖直接子进程；工具自行派生的后台进程会带着本次策略活过调用边界

spawn_tool_runner 的沙箱分支：
    受限时命令为 landstrip run -p <策略>... -- <tool_runner>，未受限时直接执行 runner
    两条分支都由 ConfinedSpawn 统一做环境最小化与工作目录设置

runner_failure 补充：stderr 里可能混入 landstrip 的拒绝事件，所以按"最后一条 kind 属于
    ToolErrorKind 的 JSON 行"取值，trap 事件不会把错误种类带偏

ToolExecutor（枚举）：工具执行器，由 agent_plugin 定义
    变体：LuaRuntime、SkillLoader、Hook
    说明：执行器是进程内的分派标签，不是资源，也没有资源 ID
        早期实现用 tool:builtin/* 这类伪造资源 ID 区分执行器，现已删除：
        伪 ID 会让人误以为它们是可导入的资源，而它们既无实体也无可解析内容
    分派：tool_call_route_system 按 executor 直接分派，不再做字符串比较

LuaError：Lua 执行限额错误，公开结构体--只用于构造期的参数校验
    kind: LuaErrorKind--稳定有限分类
    message: String--稳定有界描述
    impl Clone + PartialEq + Eq + fmt::Display + std::error::Error

LuaErrorKind：Lua 限额错误分类，公开枚举
    InvalidLimits--限额为零、互不自洽，或宿主调用时限超过总执行时限

ShellError：Shell 限额错误，公开结构体--只用于构造期的参数校验
    kind: ShellErrorKind--稳定有限分类
    message: String--稳定有界描述
    impl Clone + PartialEq + Eq + fmt::Display + std::error::Error

ShellErrorKind：Shell 限额错误分类，公开枚举
    InvalidLimits--限额为零

PreparedLuaToolCall：待执行的 Lua 工具调用，crate 内公开事件--prepare 与 execute 之间的载荷
    arguments: String--模型给出的原始参数
    handle: LuaExecutionHandle--执行上下文、限额与已内联的工具包内容
    response: LuaToolResponseGuard--兜底回执，未显式响应时由 Drop 发出失败结果
    temp_dir: CallTempDir--本次调用的临时目录守卫，随载荷释放而清理

LuaTaskError：Lua 异步任务错误，crate 内公开结构体--包装 async_runtime_plugin 的任务错误
    source: AsyncTaskError--底层任务错误
    impl From<AsyncTaskError>

install_lua_environment(lua, handle) -> Result<Table, ToolError>
    注入工具环境：私有函数，注入面只有一个入口函数
    行为：建 entry 表（version / arguments / context / json / shell）放进 Lua 注册表；
          把全局 margatroid 设为"返回该表"的函数；arguments 由 run_lua_tool 在调用前填入
    注入面：json 的 encode / decode 用 serde_json 直接实现；
            shell(command, options?) 走 runner 内的终端执行，返回 { exit_code, stdout, truncated, terminal }；
            命令为空时该函数报错，而不是返回一个空结果
    约束：不再有 fs / process / http / json / log 这些表——文件、网络、派生一律走标准库，由沙箱约束；
          命令执行没有走 io.popen，因为管道没有终端、没有可回收的进程组、也没有执行限额
    terminal 字段的用途：沙箱可能不授予 pty 设备，此时命令照样执行，只是看不到终端；
          依赖终端的工具据此自行说明，而不是在无终端时悄悄做错

run_lua_tool(request: LuaToolRunRequest) -> Result<String, ToolError>
    执行 Lua 工具：公开异步函数，插件进程内与 runner 进程共用同一段逻辑
    行为：读工具包（main.lua 与 input schema）→ 校验 arguments → 建 VM（StdLib::ALL + 内存上限 + 执行钩子 + 注入宿主面）
          → 把 arguments 填进入口表 → 加载 main.lua → 调用全局 execute → 结果长度不超过 max_output_bytes
    行为补充：工具包内容由 daemon 读取后随请求下发，沙箱内不再有进程需要读镜像根
    约束：request 携带 description、input_schema、script、temp_dir、arguments、agent_id、turn_id、
          resource_id、project_root、limits 与 sandbox_policies；
          其中 description 与 input_schema 是已解析值，不是待解析的文档；

工具结果的取值约定：
    工具可以返回 {"ok": bool, "content": string} 作为显式结果信封
        content 是模型读到的内容，模型侧不需要再解析
        ok 表示这次调用是否失败，这是单纯返回字符串无法表达的
        ok 与 content 必须同时存在，只含其中一个的 JSON 对象不算信封
        content 为空字符串仍是有效结果，不是"没有结果"
    不满足信封形状的返回值保持旧语义：
        返回值即成功，只有报错才算调用失败
    由 Lua 的 error 触发的运行时错误是工具在说"停止"，映射为 Aborted
        运行时限额触发的错误不是工具在要求停止，映射为 ExecutionFailed

失败是否结束当前轮：
    只有两种失败会结束当前轮并保留信息供人查看，因为重试无法改变它们：
        工具调用 error 声明自己无法继续
        执行工具的 runner 本身失败——无法启动、流中断或超时
    其余失败都交回模型重试：命令非零退出、脚本拒绝参数、工具报告 ok=false

    派生 runner 执行工具：私有异步函数
    行为：把请求序列化成 JSON 写 runner 的 stdin，读 stdout 作为工具结果，stderr 用于还原错误
          runner 路径取环境变量 MARGATROID_TOOL_RUNNER，缺省取当前可执行文件同目录下的 tool_runner
          超时用 limits.max_execution_time，输出上限用 limits.max_output_bytes（截断即 ExecutionFailed）
    边界：runner 无法启动、流失败或超时统一记为 RunnerFailed，与"工具执行失败"严格区分：前者结束当前轮，后者交回模型重试

runner_failure(stderr: &str) -> ToolError
    还原 runner 错误：私有函数，解析 stderr 的 JSON 并映射错误种类
    行为：kind 为 InvalidRequest / InvalidArguments / InvalidDefinition / Aborted 时保留原种类，
          其余与不可解析一律 ExecutionFailed

tool_runner_path() -> Result<PathBuf, ToolError>
    定位 runner：私有函数，先看环境变量，再看当前可执行文件同目录

集成测试定位 runner 的相对路径：
    测试与插件二进制都不带 runner，需要指向工作区构建产物
    crates/margatroid/tool_plugin 到工作区根是三级，因此 runner 位于 ../../../target/debug/tool_runner

tool_runner（src/bin/tool_runner.rs，bin 目标）：独立进程执行 Lua 工具，与上面的 spawn 路径成对
    请求：stdin 传 JSON（也接受 argv[1] 指定文件），字段与 LuaToolRunRequest 对齐，
          limits 里的时间以毫秒表示（max_execution_time_ms / max_host_call_time_ms）
    输出：工具返回值原样写 stdout
    退出码：0 成功；2 插件侧问题（InvalidRequest / InvalidArguments / InvalidDefinition）；3 工具执行失败
    stderr：非零退出时输出 {"kind": "...", "message": "..."}，供 spawn_tool_runner 还原
pty.rs   run_in_pty（分配 PTY 或退回管道执行命令；回收进程组、施加超时与输出限额）
        说明：终端不可用时退回管道而不是失败，两路输出都会合并成一路交给调用方

终端执行的四处细节：
    PTY 在最后一个 slave 关闭时以 EIO 表示流结束，这是该循环的正常结束方式，不作为错误
    终端把 stdout 与 stderr 合成一路，管道分支因此也把两者合并后交给调用方；
        stderr 先进自己的缓冲区，避免两条管道互相阻塞
    终端必须有具体的类型名，所以 pty 分支设置真实 TERM，而管道分支保留 TERM=dumb
    子进程自建会话并把 slave 设为控制终端：
        否则它会共用 runner 的会话，一个从未真正获得的终端会错误地报告自身状态
```

# events

## 类型

公开：
```text
ToolRegisterRequest：工具注册请求，公开事件
    id: String--请求 ID
    agent: Entity--目标 Agent
    resource_id: ResourceId--资源 ID
    alias: Option<String>--Agent 内别名
    impl Event for ToolRegisterRequest

ToolRegisterResponse：工具注册响应，公开事件
    id: String--原请求 ID
    agent: Entity--目标 Agent
    resource_id: ResourceId--资源 ID
    alias: Option<String>--Agent 内别名
    result: Result<ResourceMapEntry, ToolError>--注册结果
    impl Event for ToolRegisterResponse

CancelToolTurn：取消工具轮次，公开事件
    turn_id: String--轮次 ID
    agent: Entity--目标 Agent
    impl Event for CancelToolTurn

ToolCallEvent：模型工具调用事件，公开 re-export
    来自 margatroid_types，由 MclPlugin 发出
```

# types

## 类型

公开：
```text
ToolTemplate：工具模板，公开结构体
    name: String--模型可见名称
    description: String--模型可见描述
    parameters: serde_json::Value--JSON Schema
    new(name, description, parameters) -> Result<Self, ToolError>

ResourceContent：资源内容，公开枚举--导入时携带、供后续阶段使用的资源载荷
    Tool { script_path: PathBuf }--Lua 工具脚本路径
    Skill { content_path: PathBuf }--技能正文路径
    Hook--钩子无附加载荷
    Prompt { role: String, content: Arc<str> }--提示角色与正文
    Sandbox { policy: Arc<str> }--沙箱策略 JSON 文本
    说明：路径在导入时确定并写入资源实体，执行时直接读取，
        因此重新解析资源库不会影响一次已经开始的调用

ResourceMapEntry：资源映射条目，公开结构体
    resource_id: ResourceRef
    resource_name: String
    alias: Option<String>
    executor: Option<ToolExecutor>--可执行资源的执行器；prompt 与 sandbox 为空
    template: Option<ToolTemplate>
    content: Option<ResourceContent>
    说明：executor 为空且 template 为空、content 为 Prompt 或 Sandbox 时，
        该条目是资源而非可调用工具，不写入 Agent 的工具表

ToolCallRequest：工具调用请求，公开内部类型--不再作为 ECS 事件
    effect_id: String--所属效果的 ID
    turn_id: String--轮次 ID
    agent: Entity--目标 Agent
    executor: ToolExecutor--执行器标签
    resource_id: ResourceRef--资源 ID
    tool_call_id: String--工具调用 ID
    arguments: String--参数 JSON 文本
```

crate公开：
```text
validate_template(template: &ToolTemplate) -> Result<(), ToolError>
    校验工具模板：crate公开函数，描述非空且参数为对象
```

# error

## 类型

公开：
```text
ToolErrorKind：工具错误分类，公开枚举
    AgentMissing
    ResourceMapMissing
    InvalidResource
    ResourceUnavailable
    RegistrationFailed
    ToolCallMissing
    InvalidDefinition
    ProviderMissing
    ResourceResolutionFailed
    AgentNotAlive
    ToolEnvironmentMissing
    ToolPluginMissing
    ToolAlreadyRegistered
    DuplicateResource
    InvalidRequest
    InvalidArguments
    ExecutionFailed
    RunnerFailed--runner 定位、启动、管道或超时失败：环境问题，重试无益
    Aborted--工具主动调用 error，表示它无法继续

ToolError：工具错误，公开结构体--稳定分类和有界安全描述
    kind: ToolErrorKind--错误分类，私有
    message: String--有界描述，私有
    new(kind, message) -> Self
    kind(&self) -> ToolErrorKind
    message(&self) -> &str
    panic(self) -> !--crate公开方法，用于 Plugin 依赖缺失时终止
    impl Clone + Debug + PartialEq + Eq for ToolError
    impl fmt::Display for ToolError
    impl std::error::Error for ToolError
```

## 失败之后：继续还是中止

工具失败有两种去向，区别在于模型再试一次是否可能得到不同结果：

```text
AgentFailure{kind: Agent}：Aborted、RunnerFailed
    工具声明自己无法继续，或执行工具的 runner 本身坏了。二者都是环境问题，
    再调用一次不会改变结果，因此结束当前轮，错误留给人工查看。
AgentMessage::Tool{failed: true}：其余一切
    命令以非零码退出、脚本拒绝了参数、工具回了 ok = false。
    模型可以用又一次调用回应，这正是把消息交回给它的理由。
```

Lua 侧由 `ends_the_turn(ToolErrorKind)` 判定，推理侧由 `AgentFailureKind::Inference` 判定。

# 逻辑

```text
注册：
Base Lua IMPORT -> MclPlugin -> ToolRegisterRequest
    -> tool_register_system 或四类 handler 系统
    -> ToolRegisterResponse -> MclPlugin 完成 IMPORT

调用：
Base Lua EMIT EFFECT tool_call FROM ? -> MclPlugin -> ToolCallEvent
    -> tool_call_route_system
        -> skill/hook: 同步执行 -> finish_tool_call -> AgentMessage::Tool
        -> lua: prepare -> 异步执行 -> 异步 guard 发送 AgentMessage::Tool
    说明：shell 不在调用路径上；命令由 Lua 工具的 margatroid().shell 经 runner 执行
    -> AgentPlugin 投递回 Base Lua
```

# 持有关系

```text
App
└── World
    ├── ToolPluginInstalled
    ├── SkillRoots / HookRoots / LuaRoots / ShellRoots
    ├── LuaExecutionLimits
    └── Agent
        ├── resources: AgentResourceMap
        └── tools.pending: HashMap<(Entity, String, String), AgentToolPending>
