# lib

## 类型

公开：
```text
AgentPlugin：Agent数据与Lua消息入口插件，公开结构体--创建Agent Entity、路由AgentControl并把AgentMessage投递给长期Lua VM
    schedule: String--System所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开构建方法
    impl Default for AgentPlugin
        Default：公开trait实现，调用new
    impl Plugin for AgentPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：要求RuntimePlugin、ResourceIdPlugin和LuaRuntimePlugin已安装
            行为：
                重复安装时panic
                插入AgentPluginInstalled
                注册AgentInitializationCompleted内部事件
                在schedule依次挂载agent_create_system、agent_control_system、agent_message_system和agent_lua_vm_state_system；agent_lua_vm_state_system必须先于MclPlugin的Effect System运行
                不挂载可见性、推理、工具、历史、压缩或MCL Effect专用System

Agent：Agent Entity的数据图书馆，公开Component--组件存在本身表明Entity是Agent；Agent运行所需的数据统一保存在此，不保存Lua VM本体
    info: AgentInfo--静态Agent信息
    creation: AgentCreationState--创建请求和创建回执
    mcl: AgentMcl--MCL Block程序集、引用程序集和运行时数据
    resources: AgentResourceMap--资源定义、别名和解析状态
    memory: AgentMemoryHandle--历史和实时存储句柄
    inference: AgentInferenceState--模型配置和当前推理关联数据
    tools: AgentToolState--工具调用关联数据
    lua: AgentLuaState--Base Lua注册请求和长期VM状态
    lifecycle: AgentLifecycleState--生命周期状态
    turn: AgentTurnState--当前轮次和工作状态
    token_usage: TokenUsageState--累计Token用量
    last_error: Option<AgentError>--最近一次稳定错误
    info(&self) -> &AgentInfo
        读取静态信息：公开方法
    mcl(&self) -> &AgentMcl
        读取MCL数据：公开方法
    mcl_mut(&mut self) -> &mut AgentMcl
        修改MCL数据：公开方法，只能通过AgentMcl公开的封装方法操作其私有存储
    impl Component for Agent

AgentPluginInstalled：AgentPlugin安装标记，公开Resource
    impl Resource for AgentPluginInstalled
```

## 逻辑

```text
Entity识别：
    同时挂载Agent和ResourceId的Entity是可寻址Agent
    Agent组件存在本身表示Entity具有Agent语义
    ResourceId是独立统一身份组件，不在Agent.info中复制身份

数据所有权：
    Agent是Agent运行数据的唯一存储
    其他领域Plugin读取或修改Agent中自己负责的字段，不挂载第二份Agent状态组件
    Agent.mcl中的Block和RefBlock名称、顺序与内容全部由Base Lua通过MCL命令声明和修改

插件边界：
    除AgentCreateRequest外，AgentPlugin只消费AgentControl和AgentMessage两种Agent领域事件
    创建和控制使用事件内的一次性回执；AgentMessage是内部成功消息流，投递失败转为Agent终态错误
    AgentPlugin不发布AgentCreated或AgentFailure事件
    AgentPlugin不解析MCL命令、不执行工具、不构造ToolSpec、不写历史或实时数据库
    AgentPlugin不创建、执行或销毁Lua VM，只通过LuaRuntimeHandle注册、投递消息和停止VM
```

# events

## 类型

公开：
```text
AgentCreateRequest：Agent创建请求，公开事件--Workspace交付创建Agent所需的静态信息和Base Lua程序
    id: String--创建请求ID
    agent_id: ResourceId--稳定Agent资源ID，创建时作为独立Component挂载
    workspace_id: Entity--所属Workspace Entity
    base_lua: LuaProgram--Agent镜像提供的Base Lua程序
    project_root: PathBuf--项目根目录
    image_root: PathBuf--Agent镜像根目录
    home_root: PathBuf--主目录根目录
    model: AgentModelInfo--静态模型信息
    memory: AgentMemoryHandle--MemoryPlugin在创建前打开的存储句柄，必须在Base Lua启动前写入Agent.memory
    token_usage: TokenUsage--从历史恢复的累计用量
    reply: AgentCreateReply--创建完成的一次性回执
    impl Event for AgentCreateRequest

AgentControl：Agent控制事件，公开事件--唯一的非消息控制入口
    id: String--控制请求ID
    agent: Entity--目标Agent Entity
    control: AgentControlKind--控制类型和参数
    reply: AgentControlReply--一次性控制回执
    impl Event for AgentControl

AgentInitializationCompleted：Base Lua初始化完成事件，crate公开事件--MclPlugin第一次成功登记start等待后发布
    agent: Entity--已经进入消息循环的Agent
    vm_id: LuaVmId--发起start的长期VM，防止旧VM完成新实例初始化
    impl Event + Clone for AgentInitializationCompleted

AgentMessage：Agent消息事件，公开事件--共享types定义并交给目标Agent长期Lua VM的输入消息
    id: String--完整交互轮次ID
    agent: Entity--目标Agent Entity
    message: Message--User、Assistant或Tool消息；User不允许携带tool_calls
    usage: Option<TokenUsage>--只有InferencePlugin产生的Assistant响应可携带本轮用量
    impl Event for AgentMessage
```

# types

## 类型

公开：
```text
AgentCreateReply：Agent创建回执，公开结构体
    sender: Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>--私有一次性发送器槽位
    new(sender: oneshot::Sender<Result<Entity, AgentError>>) -> Self
        构造回执：公开关联函数
    send(&self, result: Result<Entity, AgentError>)
        发送结果：crate公开方法，最多发送一次

AgentControlKind：Agent生命周期控制类型，公开枚举--由agent_control_system路由到对应Handler，不承载MCL Block操作
    Stop

AgentControlReply：Agent控制回执，公开结构体
    sender: Arc<Mutex<Option<oneshot::Sender<Result<(), AgentError>>>>>--私有一次性发送器槽位
    new(sender: oneshot::Sender<Result<(), AgentError>>) -> Self
        构造回执：公开关联函数
    send(&self, result: Result<(), AgentError>)
        发送结果：crate公开方法，最多发送一次

AgentInfo：Agent静态信息，公开结构体--Base Lua通过agent_info读取的只读数据，不复制Entity的ResourceId组件
    image_entity: Entity--创建该Agent时使用的AgentImage Entity；用于读取镜像依赖清单
    workspace_id: Entity--所属Workspace
    model: AgentModelInfo--模型及上下文窗口信息
    project_root: PathBuf--项目根目录
    image_root: PathBuf--镜像根目录
    home_root: PathBuf--主目录根目录

AgentModelInfo：Agent模型信息，公开结构体
    provider: String--Provider名称
    model: String--模型名称
    context_window_tokens: u64--最大上下文窗口

AgentLifecycleState：Agent生命周期，公开枚举
    Creating
    Running
    Stopping
    Stopped
    Failed

AgentLuaState：Agent长期Lua状态，公开结构体
    request_id: Option<String>--等待LuaVmStarted时的运行时请求ID
    vm_id: Option<LuaVmId>--已经启动的长期VM标识

AgentCreationState：Agent创建状态，公开结构体
    request_id: String--AgentCreateRequest.id
    reply: AgentCreateReply--等待VM启动后的创建回执
    initialization: AgentInitializationState--Base Lua初始化及IMPORT验证状态

AgentInitializationState：Agent初始化状态，公开结构体
    failed: Option<AgentError>--首个初始化失败
    complete: bool--MclPlugin确认Base Lua第一次成功登记start后为true

AgentTurnState：Agent轮次状态，公开结构体--仅记录轮次占用，不保存上下文副本
    turn_id: Option<String>--当前轮次或压缩请求ID
    begin(&mut self, id: String) -> Result<(), AgentError>
        开始轮次：已有轮次时失败
    finish(&mut self, id: &str) -> Result<(), AgentError>
        完成轮次：只允许完成当前轮次
    abort(&mut self) -> Option<String>
        中止轮次：清空并返回当前轮次

TokenUsageState：Token用量状态，公开结构体
    total_input_tokens: u64
    total_output_tokens: u64
    total_cache_hit_tokens: u64
    cache_hit_rate: f64
    last_input_tokens: u64
    add(&mut self, usage: &TokenUsage)
        累加用量：饱和累加并重新计算命中率

```

crate公开：
```text
AgentMcl：MCL数据，公开结构体--Agent持有的MCL运行时存储，由自身封装Block和RefBlock的机械存取
    blocks: BlockAssembly--真实Block程序集，私有
    ref_blocks: RefBlockAssembly--引用Block程序集，私有
    values: HashMap<String, MclValue>--MCL运行时辅助数据，私有
    realtime_source: Option<MclRealtimeSource>--MclPlugin声明的实时上下文Message RefMerge来源；未声明时为空
    blocks(&self) -> &BlockAssembly
        读取真实Block程序集：公开方法
    ref_blocks(&self) -> &RefBlockAssembly
        读取引用Block程序集：公开方法
    select(&self, target: &BlockPath) -> Result<BlockInner, AgentError>
        查询路径：公开方法
        行为：
            target.block_id命中真实Block时，按target.inner_id读取并克隆BlockInner
            target.block_id命中RefBlock时，把target.inner_id作为merge_id并调用RefMerge迭代器返回合并后的BlockInner
            两套程序集都未命中时返回BlockMissing
            不暴露内部引用，不修改MCL数据
    merge(&self, sources: &[BlockPath]) -> Result<BlockInner, AgentError>
        真实数组合并：公开方法；逐个调用select读取源字段，允许源字段来自真实Block或RefBlock，验证类型后按声明顺序克隆拼接元素
    ref_merge(&self, sources: &[BlockPath]) -> Result<RefMerge, AgentError>
        引用路径合并：公开方法；所有路径必须命中真实Block字段且类型一致，按字段类型构造RefMerge，不读取元素；不得引用RefBlock字段
    create_block(&mut self, block_id: String, block: Block) -> Result<(), AgentError>
        创建真实Block：公开方法，block_id已存在于真实Block或RefBlock程序集时失败
    create_ref_block(&mut self, block_id: String, block: RefBlock) -> Result<(), AgentError>
        创建引用Block：公开方法，block_id已存在于真实Block或RefBlock程序集时失败
    insert(&mut self, target: &BlockPath, values: BlockInner) -> Result<(), AgentError>
        插入字段值：公开方法，找到目标Block字段，验证BlockInner类型一致后按顺序追加；整个方法原子完成
    delete(&mut self, target: &BlockPath, selection: MclDeleteSelection) -> Result<(), AgentError>
        删除字段值：公开方法，找到目标Block字段并按已经解析的删除范围移除元素；保持剩余元素顺序，整个方法原子完成
    cover(&mut self, target: &BlockPath, values: BlockInner) -> Result<(), AgentError>
        覆盖字段值：公开方法，找到目标Block字段，验证BlockInner类型一致后整体替换数组；整个方法原子完成
    realtime_source(&self) -> Option<&MclRealtimeSource>
        读取实时来源：crate公开方法，只返回当前来源描述
    set_realtime_source(&mut self, source: MclRealtimeSource)
        设置实时来源：crate公开方法，用新声明整体替换旧来源；MclPlugin必须在调用前完成RefMerge验证和当前快照展开

AgentResourceMap：Agent资源数据，crate公开结构体--实际字段与行为由ToolPlugin定义，Agent只持有唯一实例

AgentMemoryHandle：Agent存储数据，crate公开结构体--MemoryPlugin创建的可克隆存储句柄
    inner: Arc<dyn AgentMemoryStore>--历史与实时存储接口
    append_history(&self, turn_id: &str, message: &Message, tool_schema: &[ToolDefinition], usage: Option<&TokenUsage>) -> Result<(), AgentMemoryStoreError>
        追加历史：crate公开方法，转发给inner
    rewrite_realtime(&self, messages: &[MclMessage]) -> Result<(), AgentMemoryStoreError>
        覆盖实时上下文：crate公开方法，转发完整快照给inner
    read_realtime(&self) -> Result<Vec<MclMessage>, AgentMemoryStoreError>
        读取实时上下文：crate公开方法，返回完整有序快照
    impl Clone for AgentMemoryHandle

AgentMemoryStore：Agent存储接口，crate公开trait--由MemoryPlugin实现，types crate只定义协议
    继承：Send + Sync + 'static
    append_history(&self, turn_id: &str, message: &Message, tool_schema: &[ToolDefinition], usage: Option<&TokenUsage>) -> Result<(), AgentMemoryStoreError>
    rewrite_realtime(&self, messages: &[MclMessage]) -> Result<(), AgentMemoryStoreError>
    read_realtime(&self) -> Result<Vec<MclMessage>, AgentMemoryStoreError>

AgentMemoryStoreError：Agent存储接口错误，crate公开结构体--不包含数据库内容、SQL或消息正文
    kind: String--稳定有限分类，由MemoryPlugin转换为MemoryErrorKind
    message: String--稳定有界描述
    impl Clone + fmt::Display + std::error::Error for AgentMemoryStoreError

AgentInferenceState：Agent推理数据，crate公开结构体--InferencePlugin读写的模型配置和飞行中请求
    model: AgentModelInfo--当前模型静态信息
    pending: HashMap<String, AgentInferencePending>--请求ID到ToolSpec快照和取消信息

AgentToolState：Agent工具数据，crate公开结构体--ToolPlugin读写的飞行中工具调用关联
    pending: HashMap<String, AgentToolPending>--tool_call_id到执行定位和取消信息

AgentRuntimeContext：Agent运行时环境，crate公开结构体--创建时注入LuaRuntimePlugin的显式环境数据
    resource_id: ResourceId--Entity独立身份组件的副本
    agent_info: AgentInfo--Agent中的只读静态信息；环境提供器将resource_id作为id字段与其组合后注入Lua
    mcl_provider: AgentMclEnvironmentProvider--mcl阻塞调用提供器

AgentMclEnvironmentProvider：Agent MCL环境提供器，crate公开结构体--为Base Lua注入显式指定目标Agent且始终返回结果的阻塞mcl函数
    events: RuntimeEventSender--向MclPlugin发送MclCommandRequest
    name(&self) -> &str
        获取名称：返回mcl
    provide(&self, context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>
        提供MCL函数：注入mcl(target_agent_id, command, binding?)；每次调用都显式指定目标Agent，不使用context.owner作为默认目标；函数创建一次性回执并发送MclCommandRequest，随后暂停当前Lua VM并把控制权交还LuaRuntimePlugin，直到回执到达后恢复VM，将Result转换为Lua值或Lua错误并返回
        MclCommandValue::Message以及Inner(Message[])转换Lua时把MclMessage.message的type、content、reasoning、tool_calls等字段与可选usage放在同一层；Lua直接读取message.type和message.usage，不暴露Rust内部的message嵌套字段

AgentLuaMessageEnvelope：AgentPlugin写入长期VM邮箱的内部信封，crate公开结构体--start Effect消费后只把message返回Lua
    turn_id: String--原AgentMessage.id，供MclPlugin建立或校验Agent.turn
    message: MclMessage--由AgentMessage.message和usage无损组合
    impl Serialize + Deserialize for AgentLuaMessageEnvelope
```

Lua侧 `mcl` 调用约定：
```text
mcl(target_agent_id, command, binding?) -> value | error
    进入函数：创建oneshot回执，生成完整MclCommandRequest并提交事件
    等待阶段：暂停当前Lua调用栈；LuaRuntimePlugin继续驱动其他任务，不执行本调用点之后的语句
    收到回执：恢复同一Lua调用栈；成功时返回MclCommandValue转换出的Lua值，失败时抛出Lua错误
    返回值处理：Lua可以使用local value = mcl(...)接收，也可以直接调用mcl(...)忽略返回值；两者都必须等待回执
```

# system

## 函数

crate公开：
```text
agent_create_system(world: &mut World)
    创建System：crate公开System
    处理事件：AgentCreateRequest
    行为：克隆本帧全部AgentCreateRequest，逐个调用handle_agent_create

agent_control_system(world: &mut World)
    控制System：crate公开System
    处理事件：AgentControl
    行为：克隆本帧全部AgentControl，逐个调用handle_agent_control

agent_message_system(world: &mut World)
    消息System：crate公开System
    处理事件：AgentMessage
    行为：克隆本帧全部AgentMessage，逐个调用handle_agent_message

agent_lua_vm_state_system(world: &mut World)
    Lua VM状态System：crate公开System
    处理事件：LuaVmStarted、AgentInitializationCompleted、LuaRuntimeTaskFinished
    行为：
        克隆本帧全部LuaVmStarted并逐个调用handle_lua_vm_started
        克隆本帧全部AgentInitializationCompleted并逐个调用handle_agent_initialization_completed
        克隆本帧全部LuaRuntimeTaskFinished并逐个调用handle_lua_vm_finished
```

# error

## 类型

公开：
```text
AgentFailureKind：Agent错误分类，公开枚举
    InvalidRequest
    AgentMissing
    DuplicateAgent
    LuaRuntime
    Mcl
    Import
    Stopped

AgentError：Agent控制错误，公开结构体--稳定描述Agent数据、生命周期或跨领域控制失败
    kind: AgentFailureKind--错误分类
    message: String--稳定有界错误描述
```

# handler

## 函数

crate公开：
```text
handle_agent_create(world: &mut World, request: AgentCreateRequest)
    处理创建：crate公开函数
    行为：
        校验请求、Workspace和agent_id唯一性
        创建Entity并分别挂载ResourceId(agent_id)和完整Agent
        把除身份外的静态信息、image_entity、空BlockAssembly、空RefBlockAssembly、memory和TokenUsage写入Agent
        读取ResourceId并与Agent.info组合出Lua agent_info表
        确认Agent.memory已经可读后，构造providers显式包含agent_info和mcl、owner_id为agent_id的LongRunning LuaRuntimeRequest
        提交前把运行时request_id写入Agent.lua.request_id
        调用LuaRuntimeHandle::register_long_running
        Agent在Base Lua初始化完成前保持Creating，不得通过AgentCreateReply报告成功
        同步失败时把Agent.lifecycle改为Failed并通过AgentCreateReply返回错误

handle_agent_control(world: &mut World, event: AgentControl)
    处理控制：crate公开函数
    行为：
        要求目标Entity同时具有Agent和ResourceId
        按event.control调用唯一对应的生命周期函数
        立即完成时调用event.reply.send
        失败时写入Agent.last_error并调用event.reply.send(Err)

handle_agent_message(world: &mut World, event: AgentMessage)
    处理消息：crate公开函数
    行为：
        要求目标Entity同时具有Agent和ResourceId、Agent.lifecycle为Running且Agent.lua.vm_id存在
        验证User不携带tool_calls
        将event.id、event.message和event.usage组合成AgentLuaMessageEnvelope，再转换为LuaValue并调用LuaRuntimeHandle::send_message
        不解析消息角色、不维护pending_tool、不写上下文、不启动下一轮推理
        投递成功即结束；投递失败时写入Agent.last_error、把生命周期设为Failed并停止长期VM，使正在等待start的邮箱receive以错误完成

handle_lua_vm_started(world: &mut World, event: LuaVmStarted)
    处理VM启动：crate公开函数
    行为：按Agent.lua.request_id定位唯一Agent，写入vm_id、清空request_id；保持lifecycle=Creating，不提前完成创建回执

handle_agent_initialization_completed(world: &mut World, event: AgentInitializationCompleted)
    完成Base Lua初始化：crate公开函数
    行为：要求目标Agent仍为Creating、Agent.lua.vm_id等于event.vm_id且initialization.failed为空
        设置initialization.complete=true和lifecycle=Running，通过Agent.creation.reply返回Entity
        重复事件按幂等忽略；Agent、VM或生命周期不匹配时记录稳定错误且不完成错误实例的回执

handle_lua_vm_finished(world: &mut World, event: LuaRuntimeTaskFinished)
    处理VM结束：crate公开函数
    行为：
        启动前失败时按request_id定位Agent，设置lifecycle=Failed并通过创建回执返回错误
        运行后结束时按vm_id定位Agent，清空Agent.lua并根据结果设置Stopped或Failed
        失败时写入Agent.last_error

control_stop(world: &mut World, agent: Entity) -> Result<(), AgentError>
    停止Agent：crate公开函数，将Agent.lifecycle设为Stopping并调用LuaRuntimeHandle::stop_long_running
```

AgentPlugin控制边界：
```text
AgentPlugin不定义或实现visibility、conversation、history、realtime、compression、inference和tool语义
AgentPlugin不提供恢复默认可见性、删除全部可见性、中止推理或压缩上下文等专用控制
Agent只聚合AgentMcl并提供mcl与mcl_mut访问，不实现任何Block操作
AgentMcl封装BlockAssembly和RefBlockAssembly，并提供insert、delete、cover三个字段级机械修改方法
MclPlugin负责解析MCL命令、查找Agent Entity、读取Agent组件、解析绑定和删除条件，再调用AgentMcl对应的机械修改方法
AgentPlugin不得从Block名称、字段名称、内容类型或调用来源推断Block用途
AgentMcl不公开内部BlockAssembly和RefBlockAssembly的可变引用，防止其他领域绕过insert、delete、cover
真实Block和RefBlock共享同一个block_id命名空间，AgentMcl禁止两套程序集出现相同ID
Stop只结束Agent自身的长期Lua VM和生命周期，不读取或修改任何Block
```
