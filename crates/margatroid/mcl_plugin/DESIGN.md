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
# lib        src/lib.rs        模块声明与 crate 公开导出
# system     src/system.rs     System、Plugin、Lua 环境与等待型 Effect 处理
# handler    src/handler.rs    命令解析与直接操作处理函数
# events     src/events.rs     事件类型
# types      src/types.rs      命令、操作、Effect 与值类型
# error      src/error.rs      Error 类型
```

## lib

lib 只放模块声明和 crate 公开导出，不展开业务类型和函数。

## system

system 放 System、Plugin、Lua 环境、等待型 Effect 处理函数。System 只负责读取本帧事件并路由到 handler 或直接完成领域响应。

## handler

handler 放命令解析函数和直接操作处理函数。直接操作指不进入 MclDomainRequest 的 Block、REF_MERGE、INJECT、SELECT、DELETE 等命令。

## events

events 放事件类型。事件类型只包含字段和 `impl Event`，不实现业务逻辑。

## types

types 放命令、操作、Effect 和值类型，以及 MCL 程序加载函数。

## error

error 放 Error 类型和公开错误分类。

# lib

## 模块

```text
mod error;
mod events;
mod handler;
mod system;
mod types;

pub use error::MclError;
pub use events::*;
pub use handler::{
    domain_to_command, execute_direct_operation, history_append, parse_operation, realtime_load,
    realtime_source,
};
pub use system::{
    command_value_to_json, mcl_command_reply_system, mcl_command_request_system, mcl_domain_system,
    mcl_effect_response_system, mcl_import_response_system, MclPlugin, MclPluginInstalled,
    PendingMclEffects, PendingMclImports,
};
pub use types::*;
```

# system

## 类型

公开：
```text
MclPlugin：MCL 领域运行时插件，公开结构体--解析 MCL 命令、执行领域操作并完成命令回执
    schedule: String--System 所属 Schedule，私有
    new() -> Self
        构造插件：公开关联函数，使用 RuntimePlugin::UPDATE
    open(_root: impl Into<PathBuf>) -> Result<Self, MclError>
        打开插件：公开关联函数，当前忽略 root 并调用 new
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置 Schedule：公开构建方法
    impl Default for MclPlugin
        Default：公开 trait 实现，调用 new
    impl Plugin for MclPlugin
        Plugin：公开 trait 实现
        build(self, app: &mut App)
            安装插件：要求 RuntimePlugin、schedule、LuaRuntimeHandle、ResourceIdPluginInstalled 和 ToolPluginInstalled 已就绪
            行为：
                插入 MclPluginInstalled、PendingMclImports、PendingMclEffects
                向 LuaRuntimeHandle 注册 mcl 环境提供器
                依次挂载 mcl_command_request_system、mcl_domain_system、mcl_import_response_system、mcl_effect_response_system、mcl_command_reply_system

MclPluginInstalled：MclPlugin 安装标记，公开单元 Resource
    impl Resource for MclPluginInstalled

PendingMclImports：等待资源 Provider 响应的 IMPORT 事务，公开 Resource
    imports: HashMap<String, MclImportState>--AgentResourceRegisterRequest.id 到原 IMPORT 事务的唯一映射
    impl Default for PendingMclImports
    impl Resource for PendingMclImports

PendingMclEffects：等待外部结果的 Effect 事务，公开 Resource
    effects: HashMap<String, MclEffectState>--以 mcl-effect:<command_id> 为键的等待型 Effect 事务
    failures: HashMap<(Entity, String), MclError>--兼容字段；当前失败统一通过 AgentMessage(Error) 交付，不再写入暂存
    impl Default for PendingMclEffects
    impl Resource for PendingMclEffects
```

crate公开：
```text
MclEnvironmentProvider：mcl Lua 环境提供器，crate公开结构体--向 LuaRuntimePlugin 注册 mcl 函数
    events: RuntimeEventSender--用于向 MclPlugin 发送 MclCommandRequest，私有
    impl LuaEnvironmentProvider for MclEnvironmentProvider
        name(&self) -> &str
            环境名称：返回 "mcl"
        provide(&self, _context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>
            提供环境：注册全局函数 mcl(agent_id, command, binding?)
```

私有：
```text
MclHostFunction：mcl 宿主函数，私有结构体
    events: RuntimeEventSender--事件发送器，私有
    impl LuaHostFunction for MclHostFunction
        call(&self, arguments: LuaValue, _context: LuaEnvironmentContext, cancel: CancellationToken) -> HostFuture
            调用 mcl：私有方法
            行为：
                取消时返回 Cancelled
                参数必须是 2 或 3 个元素的 Lua 数组；否则 InvalidRequest
                第三个参数不为 Nil 时转换为 JSON binding
                第二个参数必须是字符串 command；第一个参数必须是可解析为 ResourceId 的字符串
                生成 MclCommandId，创建 oneshot 回执并发送 MclCommandRequest
                等待回执，错误映射为 EnvironmentFailed，成功值转换为 Lua 值
```

## 函数

公开：
```text
command_value_to_json(value: MclCommandValue) -> Result<serde_json::Value, MclError>
    MCL 值转 JSON：公开函数，用于 WebSocket mcl.command_result 和外部查询
    行为：Unit 转 null，Text 转字符串，Inner 按 BlockInner 类型转换，Message 转消息对象，Paths 转路径数组，ResourceImport 转回执对象

mcl_command_request_system(world: &mut World)
    命令请求 System：公开 System
    处理事件：MclCommandRequest
    行为：
        克隆本帧全部请求并逐个处理
        agent_id 的 resource_type 不是 agent 时回复 InvalidAgentId
        解析命令失败时回复解析错误
        Import 或 Emit 操作转发为 MclDomainRequest
        其余操作调用 execute_direct_operation 并立即回复结果

mcl_domain_system(world: &mut World)
    领域请求 System：公开 System
    处理事件：MclDomainRequest
    行为：
        克隆本帧全部请求并逐个处理
        Start 调用 begin_start；Import 调用 begin_import；RealtimeLoad 调用 begin_realtime_load；CatchInference 调用 begin_catch_inference
        上述等待型操作成功登记后不回复；同步失败发送 Err MclDomainResponse
        其余 Emit 操作按 effect 类型直接执行并发送 MclDomainResponse
        其余操作返回 EffectInvalid 并发送 MclDomainResponse

mcl_effect_response_system(world: &mut World)
    Effect 响应 System：公开 System
    处理事件：LuaVmMessageReceived、AgentRealtimeContextReadCompleted、CapturedInferenceResponse、AgentFailure
    行为：
        按响应 id 从 PendingMclEffects 移除事务，未找到则跳过
        Start 响应校验 vm_id、解析邮箱消息、建立或校验 Agent.turn、累加 token usage
        RealtimeLoad 响应包装为 BlockInner::Message
        CatchInference 响应包装为 Text
        AgentFailure 统一转换为 AgentMessage(Error) 投递给目标 Agent；Base Lua 通过 start 收到 Error 后写入历史，不回 Err、不暂存、不终止 VM

mcl_import_response_system(world: &mut World)
    IMPORT 响应 System：公开 System
    处理事件：AgentResourceRegisterResponse
    行为：
        按 response.id 从 PendingMclImports 移除事务，未找到则跳过
        校验 agent 和 resource_id 一致；不一致返回 ImportResponseMismatch
        成功时登记 alias 和 ResourceMapEntry，返回 ResourceImport 回执
        prompt 资源不调用 register_agent_resource，直接登记 resources 和 aliases
        失败时返回 ImportFailed

mcl_command_reply_system(world: &mut World)
    命令回执 System：公开 System
    处理事件：MclDomainResponse
    行为：逐个调用 response.reply.send，把 MclDomainValue 经 domain_to_command 转换后发送
```

私有：
```text
next_mcl_call_id() -> u64
    生成 MCL 调用 ID：私有函数，原子递增

command_value_to_lua(value: MclCommandValue) -> Result<LuaValue, LuaRuntimeError>
    MCL 值转 Lua：私有函数，按 MclDomainValue 变体逐项转换

mcl_message_to_json(message: MclMessage) -> Result<serde_json::Value, LuaRuntimeError>
    MCL 消息转 JSON：私有函数，Message 字段平铺并追加可选 usage；Error 输出 {type:"error",message}

json_to_lua(value: serde_json::Value) -> Result<LuaValue, LuaRuntimeError>
    JSON 转 Lua：私有函数，递归转换 JSON 值

lua_to_json_binding(value: LuaValue) -> Result<serde_json::Value, LuaRuntimeError>
    Lua 转 JSON binding：私有函数，把 lua_to_json 错误映射为 InvalidRequest

lua_to_json(value: LuaValue) -> Result<serde_json::Value, MclError>
    Lua 转 JSON：私有函数，递归转换 Lua 值

parse_mailbox_message(value: LuaValue) -> Result<AgentLuaMessageEnvelope, MclError>
    解析邮箱消息：私有函数，从 Lua 值反序列化 AgentLuaMessageEnvelope

rejected_tool_resource(world: &World, agent: Entity, call: &ToolCall) -> ResourceId
    解析被拒绝工具的资源 ID：私有函数，优先从 AgentResourceMap 按 tool_name 查找，失败时解析 tool_name，最终返回 tool:builtin/invalid

begin_start(world: &mut World, request: MclDomainRequest) -> Result<(), MclError>
    开始 start：私有函数
    行为：
        查找 Agent；存在当前 turn 时先取出并 abort 暂存失败
        要求 Agent 为 Creating 或 Running 且长期 VM 存在
        以 mcl-effect:<command_id> 登记 MclEffectState，重复 ID 返回 EffectAlreadyPending
        调用 LuaRuntimeHandle::receive_message 等待邮箱消息
        首次登记成功后若 Agent 仍 Creating 且未完成初始化，发送 AgentInitializationCompleted

begin_realtime_load(world: &mut World, request: MclDomainRequest) -> Result<(), MclError>
    开始 realtime_load：私有函数
    行为：登记 MclEffectState::RealtimeLoad 并发送 AgentRealtimeContextReadRequested

begin_catch_inference(world: &mut World, request: MclDomainRequest, ref_block_id: String) -> Result<(), MclError>
    开始 catch_inference：私有函数
    行为：
        要求 Agent 存在且当前 turn 存在
        RefBlock 必须恰好一个 Message RefMerge 且无其他类型 RefMerge
        登记 MclEffectState::CatchInference 并发送 CapturedInferenceRequest

begin_import(world: &mut World, request: MclDomainRequest, resource_id: ResourceId, alias: String) -> Result<(), MclError>
    开始 IMPORT：私有函数
    行为：
        查找 Agent 并读取 AgentInfo
        resource_id 必须在 image_dependencies 中；tool:builtin/hook 免声明
        prompt 资源直接读取镜像根目录 <name 大写>.md，scope 只允许 system 或 user，成功构造 ResourceContent::Prompt 并发送 AgentResourceRegisterResponse 完成自身
        其他资源登记 MclImportState 并发送 AgentResourceRegisterRequest
```

# handler

## 函数

公开：
```text
parse_operation(command: &str, binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    解析 MCL 命令：公开函数
    行为：
        拒绝分号；空白统一后按首词分派
        IMPORT、CREATE、SELECT、MERGE、REF_MERGE、DELETE、INJECT、EMIT EFFECT 走对应解析器
        其余返回 InvalidCommand

execute_direct_operation(world: &mut World, request: &MclCommandRequest, operation: MclOperation) -> Result<MclCommandValue, MclError>
    执行直接操作：公开函数
    行为：
        查找 Agent 并取得可变的 AgentMcl
        CreateBlock/CreateRefBlock/Merge/RefMerge/Inject/InjectMany/CoverValue/CoverInner/Select/DeleteAll/DeleteFirst/DeleteWhere 逐项执行
        修改可见性来源或默认可见性来源对应字段时，刷新 Agent.resources.visible 或 default_visible
        修改实时上下文来源依赖字段时，重新选择消息并发送 AgentRealtimeContextWriteRequested
        Import 或 Emit 返回 EffectInvalid

realtime_source(world: &mut World, agent_id: &ResourceId, ref_block_id: String) -> Result<MclDomainValue, MclError>
    声明实时来源：公开函数
    行为：
        RefBlock 必须恰好一个 Message RefMerge
        构造 MclRealtimeSource 并写入 AgentMcl.realtime_source
        展开当前快照并发送 AgentRealtimeContextWriteRequested

history_append(world: &mut World, agent_id: &ResourceId, message: MclMessage, fallback_turn_id: &str) -> Result<MclDomainValue, MclError>
    追加历史：公开函数
    行为：turn_id 取当前 turn，缺失时用 fallback_turn_id；Assistant 消息携带当前推理 tool_schema；发送 AgentHistoryMessageWriteRequested

realtime_load(world: &mut World, agent_id: &ResourceId) -> Result<MclDomainValue, MclError>
    读取实时上下文：公开函数
    行为：从 Agent.memory 读取实时上下文，过滤 System 消息后返回 BlockInner::Message

domain_to_command(value: MclDomainValue) -> MclCommandValue
    领域值转命令值：公开函数，当前直接返回 value
```

私有：
```text
parse_create(command: &str) -> Result<MclOperation, MclError>
    CREATE 解析：私有函数
    行为：
        CREATE BLOCK 支持空字段（MESSAGE/TOOL_CALL/TOOL）和 MERGE ... FROM ... AS ...
        CREATE REF_BLOCK 支持 REF_MERGE ... FROM ... AS ...
        block_id 和 inner_id 必须通过 validate_identifier
        任一字段重复或非法返回错误

parse_inject(words: &[&str], binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    INJECT 解析：私有函数
    行为：
        INJECT SELECT ... FROM ... COVER ... FROM ... 解析为 CoverInner
        其他 INJECT 按 TO/FROM 拆分，解析单个或多个值
        ? 占位符必须独占且绑定存在；多个值不允许绑定

parse_effect(words: &[&str], binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    EMIT EFFECT 解析：私有函数
    行为：
        start/finish/realtime_load 为无参 Effect
        realtime_source/inference/catch_inference 读取括号 RefBlock ID
        history_append 读取绑定 MclMessage
        visibility_source/default_visibility_source 解析 SELECT ... FROM ... 并构造 BlockPath
        tool_call 读取绑定 ToolCall 数组并校验 id/name/arguments 非空且 id 唯一

parse_delete(words: &[&str], binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    DELETE 解析：私有函数，支持全部删除、FIRST、WHERE id == ?

path(block_id: &str, inner_id: &str) -> Result<BlockPath, MclError>
    构造 BlockPath：私有函数，先校验两个标识符

validate_identifier(value: &str) -> Result<(), MclError>
    校验标识符：私有函数，要求首字符为 ASCII 字母或 _，后续为 ASCII 字母数字或 _.-，且不是 . 或 ..

required_binding(binding: Option<&serde_json::Value>) -> Result<&serde_json::Value, MclError>
    读取必需绑定：私有函数，缺失时返回 BindingMissing

reject_binding(binding: Option<&serde_json::Value>) -> Result<(), MclError>
    拒绝绑定：私有函数，存在时返回 InvalidCommand

effect_ref_block(value: &str) -> Result<String, MclError>
    解析 Effect RefBlock：私有函数，去掉括号并校验标识符

parse_message(value: &serde_json::Value) -> Result<MclMessage, MclError>
    解析消息绑定：私有函数，调用 message_from_lua_json

message_from_lua_json(value: serde_json::Value) -> Result<MclMessage, MclError>
    从 Lua JSON 解析消息：私有函数，按 type 字段解析 system/user/assistant/tool/error 消息

empty_inner(kind: InnerType) -> BlockInner
    构造空 BlockInner：私有函数，按 InnerType 返回空数组

binding_to_inner(value: &serde_json::Value, kind: InnerType, aliases: &HashMap<String, ResourceId>, sources: &HashMap<ResourceId, Arc<str>>) -> Result<BlockInner, MclError>
    绑定转 BlockInner：私有函数
    行为：
        Message 字段支持别名引用（从 sources 取内容并按 scope 构造 System/User）、消息对象或消息数组
        ToolCall 字段支持别名引用、ToolCall 数组或单个 ToolCall
        ResourceId 字段支持别名、完整资源 ID 字符串、资源 ID 数组
```

# events

## 类型

公开：
```text
MclCommandRequest：MCL 命令请求，公开事件--所有入口提交给 MclPlugin 的统一请求
    id: MclCommandId--进程内唯一命令 ID
    agent_id: ResourceId--目标 Agent 完整资源 ID
    command: String--命令文本
    binding: Option<serde_json::Value>--命令绑定值
    reply: MclCommandReply--调用方创建的一次性回执
    impl Event for MclCommandRequest
    impl Clone for MclCommandRequest

MclDomainRequest：MCL 领域请求，公开事件--命令解析 System 为 IMPORT 或 EMIT EFFECT 产生的跨领域操作
    id: MclCommandId--原命令 ID
    agent_id: ResourceId--原请求目标 Agent 资源 ID
    operation: MclOperation--已解析、尚未执行的领域操作
    reply: MclCommandReply--从命令请求原样传递的回执
    impl Event for MclDomainRequest
    impl Clone for MclDomainRequest

MclDomainResponse：MCL 领域响应，公开事件--领域操作完成后产生的类型化结果
    id: MclCommandId--原命令 ID
    agent_id: ResourceId--原请求目标 Agent 资源 ID
    result: Result<MclDomainValue, MclError>--领域结果
    reply: MclCommandReply--从领域请求原样传递的回执
    impl Event for MclDomainResponse
    impl Clone for MclDomainResponse

MclImportState：IMPORT 事务状态，公开结构体
    command_id: MclCommandId--原 MCL 命令 ID
    agent_id: ResourceId--原命令目标 Agent 资源 ID
    agent: Entity--解析出的目标 Agent Entity
    resource_id: ResourceId--待导入的完整资源 ID
    alias: String--待登记的 Agent 内别名
    reply: MclCommandReply--原命令的一次性回执，由响应链最终完成

MclEffectState：等待型 Effect 事务，公开结构体
    command_id: MclCommandId--原 MCL 命令 ID
    agent_id: ResourceId--原命令目标 Agent 资源 ID
    agent: Entity--开始 Effect 时解析出的目标 Agent Entity
    vm_id: Option<LuaVmId>--Start Effect 使用的长期 VM，其他 Effect 为空
    kind: MclPendingEffectKind--等待型 Effect 分类
    reply: MclCommandReply--原命令的一次性回执，由响应链最终完成
```

# types

## 类型

公开：
```text
MclHash：MCL 源码哈希，公开结构体
    as_str(&self) -> &str
        读取哈希：公开方法
    impl Clone + Debug + PartialEq + Eq + PartialOrd + Ord + Hash for MclHash
    impl fmt::Display for MclHash

MclProgramKind：MCL 程序类型，公开枚举
    Base
    Workflow
    Module

MclSource：MCL 源文件，公开结构体
    resource_id: ResourceId--资源 ID，私有
    source: Arc<str>--源码，私有
    origin: Arc<PathBuf>--来源路径，私有
    new(resource_id: ResourceId, source: impl Into<Arc<str>>, origin: impl Into<Arc<PathBuf>>) -> Self
        构造源：公开关联函数
    resource_id(&self) -> &ResourceId
        读取资源 ID：公开方法
    source(&self) -> &str
        读取源码：公开方法
    origin(&self) -> &Path
        读取来源路径：公开方法

MclProgram：MCL 程序，公开结构体
    source(&self) -> &str
        读取源码：公开方法
    origin(&self) -> &Path
        读取来源路径：公开方法
    resource_id(&self) -> &ResourceId
        读取资源 ID：公开方法
    kind(&self) -> MclProgramKind
        读取程序类型：公开方法
    source_hash(&self) -> &MclHash
        读取源码哈希：公开方法
    plan_hash(&self) -> &MclHash
        读取计划哈希：公开方法

MclCompileRequest：MCL 编译请求，公开结构体
    root: MclSource--根源文件
    dependencies: BTreeMap<ResourceId, MclSource>--依赖源文件

ResourceImportReceipt：资源导入回执，公开结构体
    resource_id: ResourceId--资源 ID
    alias: String--Agent 内别名
    available: bool--是否可用
    error: Option<String>--可选错误
    impl Clone + Debug + PartialEq + Eq for ResourceImportReceipt

MclCommandId：MCL 命令 ID，公开结构体
    new(value: impl Into<String>) -> Result<Self, MclError>
        构造命令 ID：公开关联函数，空值返回 InvalidCommand
    as_str(&self) -> &str
        读取命令 ID：公开方法

MclCommandReply：MCL 命令回执，公开结构体
    new(sender: oneshot::Sender<Result<MclCommandValue, MclError>>) -> Self
        构造回执：公开关联函数
    send(&self, result: Result<MclCommandValue, MclError>)
        发送结果：公开方法，最多发送一次

MclBinding：MCL 绑定值，公开结构体，包装 serde_json::Value

MclPredicate：MCL 删除谓词，公开枚举
    IdEquals(String)

BlockFieldDeclaration：Block 字段声明，公开枚举
    Empty { inner_id: String, inner_type: InnerType }
    Merge { inner_id: String, sources: Vec<BlockPath> }

RefMergeDeclaration：RefBlock 合并声明，公开结构体
    merge_id: String--合并 ID
    sources: Vec<BlockPath>--引用路径

MclEffectCommand：MCL Effect 命令，公开枚举
    Start
    CatchInference { ref_block_id: String }
    Inference { ref_block_id: String }
    ToolCall { calls: Vec<ToolCall> }
    Finish
    HistoryAppend { message: MclMessage }
    RealtimeSource { ref_block_id: String }
    VisibilitySource { source: BlockPath }
    DefaultVisibilitySource { source: BlockPath }
    RealtimeLoad

MclOperation：MCL 操作，公开枚举
    CreateBlock { block_id: String, fields: Vec<BlockFieldDeclaration> }
    CreateRefBlock { block_id: String, merges: Vec<RefMergeDeclaration> }
    Merge { sources: Vec<BlockPath> }
    RefMerge { sources: Vec<BlockPath> }
    Import { resource_id: ResourceId, alias: String }
    Inject { target: BlockPath, value: MclBinding }
    InjectMany { target: BlockPath, values: Vec<MclBinding> }
    CoverValue { target: BlockPath, value: MclBinding }
    CoverInner { source: BlockPath, target: BlockPath }
    Select { source: BlockPath }
    DeleteAll { target: BlockPath }
    DeleteFirst { target: BlockPath }
    DeleteWhere { target: BlockPath, predicate: MclPredicate }
    Emit { effect: MclEffectCommand }

MclDomainValue：MCL 领域值，公开枚举
    Unit
    Inner(BlockInner)
    Paths(Vec<BlockPath>)
    Message(MclMessage)
    ResourceImport(ResourceImportReceipt)
    Text(String)

MclCommandValue：MCL 命令值，公开类型别名，等于 MclDomainValue

MclEffect：MCL Effect，公开枚举
    Start
    CatchInference { messages: Vec<MclMessage> }
    Inference { messages: Vec<MclMessage>, visible_resources: Vec<ResourceId> }
    ToolCall { calls: Vec<ToolCall> }
    Finish
    HistoryAppend { message: MclMessage }
    RealtimeSource { source: MclRealtimeSource, values: Vec<MclMessage> }
    RealtimeLoad

MclPendingEffectKind：等待型 Effect 分类，公开枚举
    Start { vm_id: LuaVmId }
    CatchInference
    RealtimeLoad
```

## 函数

公开：
```text
compile_mcl(request: MclCompileRequest) -> Result<Arc<MclProgram>, MclError>
    编译 MCL：公开函数，当前只计算 root.source 的 SHA-256 哈希并保存 source_hash 和 plan_hash

load_mcl_program_from_path(_roots: &[PathBuf], resource_id: &ResourceId, path: &Path, expected: MclProgramKind) -> Result<Arc<MclProgram>, MclError>
    从路径加载 MCL 程序：公开函数
    行为：
        有界读取 UTF-8 源码，读取失败返回 SourceReadFailed，非 UTF-8 返回 SourceInvalidUtf8
        调用 compile_mcl 构造程序
        程序类型与 expected 不一致时返回 InvalidProgramKind
```

# error

## 类型

公开：
```text
MclError：MCL 错误，公开枚举
    ParseFailed
    InvalidAgentId
    AgentMissing
    DuplicateAgent
    AgentRuntimeMissing
    BlockMissing { assembly: String, block: String }
    InnerMissing { block: String, inner: String }
    RefBlockMissing { assembly: String, block: String }
    MergeMissing { block: String, merge: String }
    TypeMismatch
    BindingMissing
    InvalidCommand
    ImportMissing(String)
    ImportFailed
    ImportResponseMismatch
    MessageSourceUnavailable
    EffectAlreadyPending
    EffectResponseMismatch
    TurnMissing
    TurnMismatch
    MailboxFailed
    InferenceFailed
    ToolCallInvalid
    RealtimeReadFailed
    EffectInvalid
    SourceReadFailed
    InvalidResourceId
    SourceTooLarge
    SourceInvalidUtf8
    ImportCycle
    InvalidProgramKind
    impl Clone + Debug + PartialEq + Eq for MclError
    impl fmt::Display for MclError
    impl std::error::Error for MclError
```

# 逻辑

```text
Lua mcl 调用：
    Base Lua -> mcl(agent_id, command, binding?)
    MclHostFunction -> 生成 MclCommandRequest -> MclPlugin
    mcl_command_request_system 解析命令
        Import/Emit -> MclDomainRequest -> mcl_domain_system
        其他 -> execute_direct_operation -> 立即完成回执
    mcl_domain_system 按 Effect 类型执行：
        Start/CatchInference/RealtimeLoad -> 登记 PendingMclEffects 并等待外部响应
        Import -> 登记 PendingMclImports 并发送 AgentResourceRegisterRequest
        HistoryAppend/RealtimeSource/Inference/ToolCall/VisibilitySource/DefaultVisibilitySource/Finish -> 直接执行
    外部响应经 mcl_import_response_system 或 mcl_effect_response_system 完成 MclDomainResponse
    mcl_command_reply_system 把 MclDomainResponse 转换后发送原命令回执
```

# 持有关系

```text
App
└── World
    ├── PendingMclImports Resource
    │   └── imports: HashMap<String, MclImportState>
    ├── PendingMclEffects Resource
    │   ├── effects: HashMap<String, MclEffectState>
    │   └── failures: HashMap<(Entity, String), MclError>
    └── LuaRuntimeHandle
        └── mcl provider -> MclEnvironmentProvider
