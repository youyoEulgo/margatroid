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

handler 放命令解析函数和直接操作处理函数。直接操作指不进入 MclDomainRequest 的 Block、REF_MERGE、INJECT、GET、BIND、LOAD 等命令。

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
    domain_to_command, execute_direct_operation, history_append, parse_operation,
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
        call(&self, arguments: LuaValue, context: LuaEnvironmentContext, cancel: CancellationToken) -> HostFuture
            调用 mcl：私有方法
            行为：
                取消时返回 Cancelled
                参数必须是 2 或 3 个元素的 Lua 数组；否则 InvalidRequest
                第三个参数不为 Nil 时转换为 JSON binding
                第二个参数必须是字符串 command；第一个参数必须是可解析为 ResourceId 的字符串
                生成 MclCommandId
                以 context.owner.owner_id 作为 source 创建并发送 MclCommandRequest
                等待回执，错误映射为 EnvironmentFailed，成功值转换为 Lua 值
            边界：source 只取 context.owner（daemon 创建 VM 时填入，Lua 无法伪造）；第一个参数只作目标，不充当来源

MclAudit：MCL 审计元组，私有结构体--目前唯一消费者是日志
    source: &str--来源：MclCommandRequest.source；driver VM 的归属即所属 agent 的资源 id
    target: &str--目标：指令作用的 agent 资源 id，取 MclCommandRequest.agent_id
    command: &str--指令原文
    level(&self) -> tracing::Level
        审计级别：私有方法
        行为：首个关键字为 IMPORT 或 EMIT 时返回 INFO（越出 VM），否则返回 DEBUG（只改内存中的 Block）
        边界：只看首个关键字，不解析指令本身，也不判断指令是否合法
              DEBUG 默认被过滤——daemon 以 config.toml 的 log.level（默认 info）和可选的 log.filter
              构造 LogPlugin，而 log_plugin 的 build_filter 用该配置构造 EnvFilter 并作用于包括
              出站日志流在内的所有 layer，因此高频 Block 操作不进控制台与面板；全量审计可在
              config.toml 设 log.level = "debug"，或用 log.filter = "info,mcl_plugin::system=debug"
              只放开这里
    log(&self, command_id: &str)
        输出审计：私有方法
        行为：按 level 选择 info 或 debug，记录 source、target、mcl、mcl_id 四个字段
        边界：不记录 binding（体积不可控且可能含消息正文）；tracing 的事件级别必须是编译期常量，故按级别分支
              走 tracing 宏而不是 EventLog：本审计在 mcl_plugin 的请求 System 中产出，不需要 ECS 传播语义，
              而 EventLog 使用固定 target 且不携带字段——固定 target 会失去按 target 过滤的能力
              （"只看 MCL 审计"依赖 mcl_plugin::system=debug），无字段则只能把 source/target
              拼进 message，违反 EventLog 的"上下文需要时扩展而不是解析 message"约束
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
        逐个调用 audit_mcl_request 产出审计日志；审计先于目标校验
        agent_id 的 resource_type 不是 agent 时回复 InvalidAgentId
        解析命令失败时回复解析错误
        Import 或 Emit 操作转发为 MclDomainRequest
        其余操作调用 execute_direct_operation 并立即回复结果

mcl_domain_system(world: &mut World)
    领域请求 System：公开 System
    处理事件：MclDomainRequest
    行为：
        克隆本帧全部请求并逐个处理
        Start 调用 begin_start；Import 调用 begin_import；CatchInference 调用 begin_catch_inference
        上述等待型操作成功登记后不回复；同步失败发送 Err MclDomainResponse
        其余 Emit 操作按 effect 类型直接执行并发送 MclDomainResponse
        其余操作返回 EffectInvalid 并发送 MclDomainResponse

mcl_effect_response_system(world: &mut World)
    Effect 响应 System：公开 System
    处理事件：LuaVmMessageReceived、CapturedInferenceResponse、AgentFailure
    行为：
        按响应 id 从 PendingMclEffects 移除事务，未找到则跳过
        Start 响应校验 vm_id、解析邮箱消息、建立或校验 Agent.turn、累加 token usage
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
audit_mcl_request(request: &MclCommandRequest)
    审计请求：私有函数
    行为：取 request.source 为来源、request.agent_id 为目标、request.command 为指令、request.id 为命令 ID，
          构造 MclAudit 并交给日志输出
    边界：在命令解析和目标校验之前调用，因此非法目标的请求同样被审计

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
        IMPORT、CREATE、MERGE、REF_MERGE、GET、INJECT、BIND、LOAD、EMIT EFFECT 走对应解析器
        其余返回 InvalidCommand

execute_direct_operation(world: &mut World, request: &MclCommandRequest, operation: MclOperation) -> Result<MclCommandValue, MclError>
    执行直接操作：公开函数
    行为：
        查找 Agent 并取得可变的 AgentMcl
        CreateBlock/CreateRefBlock/Merge/RefMerge/Get/Inject/BindState/LoadState 逐项执行
        修改可见性来源或默认可见性来源对应字段时，刷新 Agent.resources.visible 或 default_visible
        Import 或 Emit 返回 EffectInvalid

history_append(world: &mut World, agent_id: &ResourceId, message: MclMessage, fallback_turn_id: &str, source: &str) -> Result<MclDomainValue, MclError>
    追加历史：公开函数
    行为：turn_id 取当前 turn，缺失时用 fallback_turn_id；Assistant 消息携带当前推理 tool_schema；发送 AgentHistoryMessageWriteRequested 并带上 source

history_record(world: &mut World, agent_id: &ResourceId, kind: String, content: String, payload: String, source: &str) -> Result<MclDomainValue, MclError>
    追加显式记录：公开函数
    行为：按发起者给出的 kind、content、payload 发送 AgentHistoryRecordWriteRequested，与消息共用同一时间线

setting_source(world: &mut World, agent_id: &ResourceId, ref_block_id: String) -> Result<MclDomainValue, MclError>
    声明配置来源：公开函数
    行为：要求 RefBlock 全部由 RESOURCE merge 组成，把展开的路径记录到 Agent.resources.setting_sources；不发送事件

domain_to_command(value: MclDomainValue) -> MclCommandValue
    领域值转命令值：公开函数，当前直接返回 value
```

私有：
```text
parse_create(command: &str) -> Result<MclOperation, MclError>
    CREATE 解析：私有函数
    行为：
        CREATE BLOCK 支持空字段（MESSAGE/RESOURCE）和 MERGE ... FROM ... AS ...
        CREATE REF_BLOCK 支持 REF_MERGE ... FROM ... AS ...
        block_id 和 inner_id 必须通过 validate_identifier
        任一字段重复或非法返回错误

parse_inject(words: &[&str], binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    INJECT 解析：私有函数
    行为：
        INJECT 按 `INJECT source TO target` 解析，source 和 target 均可使用完整、单值或范围选择器
        普通 Block 可作为右值，RefBlock 只能作为 source 左值；target 无索引时整体覆盖，单索引时插入，范围时替换范围
        [] 解析为空序列，用于清空数组或删除范围；? 读取绑定值，缺少 binding 返回 BindingMissing

parse_selector(value: &str) -> Result<MclSelector, MclError>
    解析选择器：私有函数
    行为：
        支持 block.inner、block.inner[index] 和 block.inner[start, end]
        非负范围要求 0 <= start < end；负数范围要求 start < end < 0；两端不得异号
        单索引可正可负，整数文本由 Lua 侧计算后拼入

selector_path(selector: &MclSelector) -> BlockPath
    读取选择器路径：私有函数

get_selector_value(mcl: &AgentMcl, selector: &MclSelector) -> Result<MclDomainValue, MclError>
    读取选择器结果：私有函数，单索引返回单个 Message 或 Unit，其余返回 BlockInner

select_selector(mcl: &AgentMcl, selector: &MclSelector) -> Result<BlockInner, MclError>
    读取选择器切片：私有函数，按完整、单索引或范围返回同类型内积

resolve_index(index: i64, length: usize) -> Result<usize, MclError>
    解析单索引：私有函数，负数从末尾计算，越界返回 TypeMismatch

resolve_range(start: i64, end: i64, length: usize) -> Result<(usize, usize), MclError>
    解析范围：私有函数，非负为左闭右开，负数为左开右闭，越界返回 TypeMismatch

slice_inner(values: &BlockInner, start: usize, end: usize) -> Result<BlockInner, MclError>
    切片内积：私有函数，保持元素类型

append_inner(target: &mut BlockInner, values: BlockInner) -> Result<(), MclError>
    追加内积：私有函数，类型不一致返回 TypeMismatch

normalize_command(command: &str) -> String
    规范化命令：私有函数，折叠空白但保留选择器内部的索引与逗号

parse_effect(words: &[&str], binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    EMIT EFFECT 解析：私有函数
    行为：
        start/finish 为无参 Effect
        inference/catch_inference 读取括号 RefBlock ID
        history_append 读取绑定 MclMessage
        visibility_source/default_visibility_source 读取括号 BlockPath
        tool_call 读取绑定 ToolCall 数组并校验 id/name/arguments 非空且 id 唯一

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
        ToolCall 已从 MCL Block 类型移除；tool_call effect 仍读取绑定的 ToolCall 数组
        ResourceId 字段支持别名、完整资源 ID 字符串、资源 ID 数组
```

# events

## 类型

公开：
```text
MclCommandRequest：MCL 命令请求，公开事件--所有入口提交给 MclPlugin 的统一请求
    id: MclCommandId--进程内唯一命令 ID
    source: String--发起方稳定标识；driver 入口填所属 Agent 资源 ID，客户端入口填客户端来源标识
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

MclSelector：MCL 选择器，公开枚举--作用于单个 Block inner
    All(BlockPath)--完整数组，等价于 [0, length]
    Index { path: BlockPath, index: i64 }--单个元素，负数从末尾计算
    Range { path: BlockPath, start: i64, end: i64 }--范围，非负为左闭右开、负数为左开右闭

MclInjectSource：INJECT 左值来源，公开枚举
    Selector(MclSelector)--来自同一或其他 Block 的选择器结果
    Bindings(Vec<MclBinding>)--来自绑定值、别名或 [] 空序列

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
    ToolCall { calls: Vec<ToolCall> }--tool_call effect 的参数协议，不属于 MCL Block 类型
    Finish
    HistoryAppend { message: MclMessage }
    HistoryRecord { kind: String, content: String, payload: String }
    SettingSource { ref_block_id: String }
    VisibilitySource { source: BlockPath }
    DefaultVisibilitySource { source: BlockPath }

MclOperation：MCL 操作，公开枚举
    CreateBlock { block_id: String, fields: Vec<BlockFieldDeclaration> }
    CreateRefBlock { block_id: String, merges: Vec<RefMergeDeclaration> }
    Merge { sources: Vec<BlockPath> }
    RefMerge { sources: Vec<BlockPath> }
    Import { resource_id: ResourceId, alias: String }
    Get { selector: MclSelector }
    Inject { source: MclInjectSource, target: MclSelector }
    BindState { block_id: String, state_name: String }
    LoadState { state_name: String, block_id: String }
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
    SettingSource { values: Vec<ResourceId> }

MclPendingEffectKind：等待型 Effect 分类，公开枚举
    Start { vm_id: LuaVmId }
    CatchInference
    ```

## STATE 语义

```text
BIND <block> TO STATE <name>
    要求 block 是普通 Block；同一 state 只能绑定一个 block，重复绑定同一对是幂等
    绑定后，block 任一字段被修改时，MCL 序列化完整 block 并写入 state
LOAD STATE <name> INTO <block>
    读取 state blob；不存在时保持 block 当前默认值
    state 中存在且当前 block 也存在的字段按类型覆盖；未知字段忽略
    state 存在但字段为空数组时覆盖默认值为空数组

```

history_messages 与 state 是两条独立通道：`history_append` 显式写入对话时间线，state 写入保存在 `setting` 表中，两者互不自动同步。实时上下文不再有专用 source effect；driver 声明普通 `realtime_state` block，通过 `LOAD`、`INJECT` 和 `BIND` 自行维护。

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
    MclHostFunction -> 以 context.owner.owner_id 为 source 生成 MclCommandRequest -> MclPlugin
客户端 mcl 调用：
    DtoPlugin -> 以客户端来源标识为 source 生成 RouteMclCommand
        -> WorkspacePlugin 解析目标 Agent -> MclCommandRequest
统一来源审计：
    mcl_command_request_system -> audit_mcl_request -> 日志输出
        两个入口共用同一实现；新增入口只需填好 source
    mcl_command_request_system 解析命令
        Import/Emit -> MclDomainRequest -> mcl_domain_system
        其他 -> execute_direct_operation -> 立即完成回执
    mcl_domain_system 按 Effect 类型执行：
        Start/CatchInference -> 登记 PendingMclEffects 并等待外部响应
        Import -> 登记 PendingMclImports 并发送 ToolRegisterRequest
        HistoryAppend/Inference/ToolCall/VisibilitySource/DefaultVisibilitySource/Finish -> 直接执行
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
