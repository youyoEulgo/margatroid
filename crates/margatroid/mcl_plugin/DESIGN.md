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

正文按lib、system、handler、events、types、error六板块组织，按实际源码位置说明，不假称Plugin或函数已迁移。新架构另见DESIGN-NEXT.md，尚未实现。

本文对应当前源码，不是下一代资源架构。`ResourceRef` 是传递、解析及保存的引用值；`ResourceId` 是 ECS 身份组件。`BlockInner::ResourceId`、`InnerType::ResourceId`、`RefMerge::ResourceId` 是现有变体名，不能据此把引用值误写成 ResourceId。

# lib

## 模块与公开导出

```text
mod error;
mod events;
mod handler;
mod system;
mod types;
pub use error::MclError;
pub use events::*;
pub use handler::{domain_to_command, execute_direct_operation, history_append, history_record, parse_operation};
pub use system::{command_value_to_json, mcl_command_reply_system, mcl_command_request_system,
    mcl_domain_system, mcl_effect_response_system, mcl_import_response_system,
    MclPlugin, MclPluginInstalled, PendingMclEffects, PendingMclImports};
pub use types::*;
```

Plugin 定义在 system；handler/system 模块自身私有，列出的 API 从 crate 根重新导出。

# system

## 类型

公开：

```text
PendingMclImports：公开结构体
    imports: HashMap<String, crate::MclImportState>--键 mcl-import:<command_id>，公开
    impl Default for PendingMclImports--空映射
    impl Resource for PendingMclImports
PendingMclEffects：公开结构体
    effects: HashMap<String, crate::MclEffectState>--推理键 turn_id；工具键 mcl-effect:<command_id>，公开
    impl Default for PendingMclEffects--空映射，没有 failures 字段
    impl Resource for PendingMclEffects
MclPluginInstalled：公开单元结构体
    impl Resource for MclPluginInstalled
MclPlugin：公开结构体
    schedule: String--私有
    new() -> Self
        构造插件：公开关联函数
        行为：使用 RuntimePlugin::UPDATE。
    open(_root: impl Into<std::path::PathBuf>) -> Result<Self, MclError>
        构造插件：公开关联函数
        行为：忽略 root，返回 Ok(Self::new())，不打开文件。
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法
        行为：替换 schedule 后返回 self。
    impl Default for MclPlugin
        default() -> Self
            默认构造：公开trait方法
            行为：调用 new。
    impl Plugin for MclPlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：要求 RuntimeHandle、指定 Schedule、LuaRuntimeHandle、ResourceIdPluginInstalled、ToolPluginInstalled 存在，否则 panic；插入安装标记和两个空 pending Resource；注册 MclEnvironmentProvider（失败 expect/panic）；依次添加 command_request、domain、import_response、effect_response、command_reply 五个系统。
```

## 类型

crate公开：

```text
MclEnvironmentProvider：MCL环境提供器，crate公开结构体
    events: RuntimeEventSender--私有
    runtime: LuaRuntimeHandle--私有
    impl LuaEnvironmentProvider for MclEnvironmentProvider
        name(&self) -> &str
            读取环境名称：公开trait方法
            行为：返回 "mcl"。
        provide(&self, _context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>
            提供环境：公开trait方法
            行为：注册 globals mcl、mcl_expose、mcl_wait、mcl_reserve_wait、mcl_yield，分别对应下列宿主函数；modules 为空。
```

私有：
```text
MclHostFunction：MCL宿主调用，私有结构体
    events: RuntimeEventSender--私有
    impl LuaHostFunction for MclHostFunction
        call(&self, arguments: LuaValue, context: LuaEnvironmentContext, cancel: CancellationToken) -> HostFuture
            执行宿主调用：公开trait方法
            行为：预先取消返回 Cancelled；要求长度 2/3 的 Array；第三项非 Nil 时转 JSON binding，前两项必须是可解析 ResourceRef 的目标 agent 字符串和 command 字符串；生成 lua-mcl-<计数> ID/oneshot，以 context.owner.owner_id 为 source 发送 MclCommandRequest；取消或等待回执，通道关闭为 RuntimeClosed，MclError 为 EnvironmentFailed，成功值转 Lua。
            限制：source 来自宿主 owner，不从目标参数取；取消等待不撤销已发送命令。
MclExposeFunction：私有结构体
    events: RuntimeEventSender--私有
    impl LuaHostFunction for MclExposeFunction
        call(&self, arguments: LuaValue, context: LuaEnvironmentContext, cancel: CancellationToken) -> HostFuture
            执行宿主调用：公开trait方法
            行为：预先检查取消，要求两项 Array(domain 字符串、mapping)；mapping 转 JSON；目标从 owner_id 解析，source 同 owner_id；生成 ID 并发送 command="EXPOSE <domain>" 请求，以与 mcl 相同方式等待和转换回执。
MclWaitFunction：私有结构体
    runtime: LuaRuntimeHandle--私有
    impl LuaHostFunction for MclWaitFunction
        call(&self, arguments: LuaValue, context: LuaEnvironmentContext, cancel: CancellationToken) -> HostFuture
            执行宿主调用：公开trait方法
            行为：只接受 Nil 或空 Array；以 owner_id claim_suspension；取消则 forget_suspension/Cancelled，接收器关闭为 RuntimeClosed，唤醒成功返回 Nil。
MclReserveWaitFunction：私有结构体
    runtime: LuaRuntimeHandle--私有
    impl LuaHostFunction for MclReserveWaitFunction
        call(&self, arguments: LuaValue, context: LuaEnvironmentContext, _cancel: CancellationToken) -> HostFuture
            执行宿主调用：公开trait方法
            行为：只接受 Nil 或空 Array；以 owner_id reserve_suspension，成功返回 Nil；忽略取消令牌。
MclYieldFunction：私有结构体
    runtime: LuaRuntimeHandle--私有
    impl LuaHostFunction for MclYieldFunction
        call(&self, arguments: LuaValue, context: LuaEnvironmentContext, _cancel: CancellationToken) -> HostFuture
            执行宿主调用：公开trait方法
            行为：只接受 Nil 或空 Array；以 owner_id drop_reservation，返回 Nil；不是调度器 yield，忽略取消令牌。
MclAudit<'a>：私有结构体，Clone + Copy + Debug
    source: &'a str--私有
    target: &'a str--私有
    command: &'a str--私有
    level(&self) -> tracing::Level
        确定审计级别：私有方法
        行为：split_whitespace 首词 IMPORT/EMIT 返回 INFO，其余 DEBUG，不判断合法性。
    log(&self, command_id: &str)
        记录审计：私有方法
        行为：按级别走 tracing::info!/debug!，记录 source、target、mcl、mcl_id，不记录 binding。
```

审计先于目标校验和命令解析，非法请求也被审计。DEBUG 的实际输出由外部 tracing 过滤配置控制；这里不发 EventLog，也不做来源授权。

## 函数

公开：

```text
command_value_to_json(value: crate::MclCommandValue) -> Result<serde_json::Value, MclError>
    转换命令 JSON：公开函数
    行为：Unit=null，Text=字符串，Message/Message inner=消息对象/数组，ResourceId inner=引用字符串数组，Paths=[{block_id,inner_id}]，ResourceImport={resource_id,alias,available,error}；消息/序列化错误转 TypeMismatch。
mcl_command_request_system(world: &mut World)
    命令请求System：公开System
    处理事件：MclCommandRequest
    产出事件：MclDomainRequest（仅 Import/Emit）；其他结果直接回 oneshot。
    行为：克隆本帧请求逐个审计；目标 resource_type 不是 agent 则 InvalidAgentId；parse_operation 失败立即回复；Import/Emit 携带 source/原回执转领域请求，其余 execute_direct_operation 并直接回复。
mcl_domain_system(world: &mut World)
    领域System：公开System
    处理事件：MclDomainRequest
    产出事件：MclDomainResponse、ToolRegisterRequest、InferenceRequestEvent、ToolCallEvent、MclEffectReply、AgentInitializationCompleted、AgentHistoryMessageWriteRequested、AgentHistoryRecordWriteRequested（含 helper 调用）。
    行为：
        克隆请求逐个分派；Wake/Ready helper 直接完成成功回执；Import/Inference 成功登记等待，失败发 Err 领域响应；其他同步结果发领域响应，ToolCall 登记等待后不发即时成功。
        HistoryAppend/HistoryRecord：调用 handler 对应函数，传 source。
        SandboxUse：别名查 aliases，找不到时尝试 ResourceRef 解析；必须 sandbox 且资源库查询成功，否则 ImportMissing/TypeMismatch；全部验证后以 BTreeSet 替换 active_sandboxes，空序列清空。
        StartTurn：要求 Agent 无当前 turn，否则 TurnMismatch；生成 mcl-turn:<计数> 并 turn.begin，失败 TurnMismatch。
        ToolCall：要求当前 turn、非空 calls/id；allowed_tools 优先当前 inference.pending 的 tool_schema 名称（即使为空），无 pending 才取 visible 对应已注册 tools；登记 ToolCall pending，重复 effect_id/缺 Resource 为 EffectAlreadyPending；允许的调用发 ToolCallEvent，拒绝的调用发 Ok(Message::Tool{failed:true}) 的 MclEffectReply，不是执行 builtin 工具兜底。
        VisibilitySource/DefaultVisibilitySource：select 完整字段，必须 ResourceId inner；设置对应集合及 source 路径。
        Finish：Agent 有 turn 且 tools.pending 有该 turn 的键则 EffectInvalid，否则 turn.finish（失败 TurnMismatch）；无 turn 或 entity 无 Agent 组件时成功 Unit。
        非领域操作返回 EffectInvalid；保留 Emit 兜底 Unit 分支，当前 effect 枚举已全部显式处理。
mcl_effect_response_system(world: &mut World)
    Effect响应System：公开System
    处理事件：margatroid_types::MclEffectReply、CapturedInferenceResponse、AgentFailure（按此顺序）。
    产出事件：MclDomainResponse、AgentConversationObserved。
    行为：
        按 MclEffectReply.id 移除 pending，缺失忽略；agent 不符则 EffectResponseMismatch；Err(String) 则 EffectFailed(String)。Inference 成功累加可选 usage，发 AgentConversationObserved，返回带 usage 的 Message。ToolCall 成功须 Tool 消息，未知 call id/重复响应把 state 放回并忽略；新响应清理对应 agent.tools.pending，收集至 expected_calls.len()，按原 ID 顺序返回 Message inner（usage=None），未齐全放回 pending。
        CapturedInferenceResponse 同样按 id 移除 pending、校验 agent；成功返回 Text，失败 warning 后返回空 Text。
        AgentFailure 查 ResourceId 组件仅用于日志标签；kind.ends_the_turn() 时 abort turn；warning 并发带 Error 的 AgentConversationObserved，不完成 pending、不投递邮箱。
    限制：CapturedInferenceResponse 是兼容读取分支，本插件无捕获式推理发起路径/CatchInference 变体，此分支不校验 pending.kind；MclEffectReply 不校验 vm_id/current turn；ToolCall.expected 未用于完成判断；没有超时清理/failures 暂存。
mcl_import_response_system(world: &mut World)
    IMPORT响应System：公开System
    处理事件：ToolRegisterResponse
    产出事件：MclDomainResponse
    行为：按 response.id 移除 IMPORT state，缺失忽略；校验 agent/resource_id，不符 ImportResponseMismatch；Provider Err 丢弃细节转 ImportFailed；成功强制 entry.alias/resource_name 为原 alias，检查 alias 冲突；无冲突时 imported_resource_definition/install_resource；prompt/sandbox 在 executor/template 均 None 且 content 匹配时跳过 register_agent_resource，其他 entry 注册；成功写 resources[reference]=true/aliases，返回 available=true/error=None 回执；失败 ImportFailed。
    限制：先安装资源库后注册 Agent，失败不回滚；不额外校验返回 entry.resource_id 与原事务一致。
mcl_command_reply_system(world: &mut World)
    命令回执System：公开System
    处理事件：MclDomainResponse
    产出事件：无，完成 oneshot。
    行为：克隆响应，result.map(domain_to_command) 后 reply.send。
```

## 函数

私有：

```text
audit_mcl_request(request: &MclCommandRequest)
    审计命令请求：私有函数
    行为：将目标引用转字符串，构造 MclAudit，以 request.id 记录 source/target/原文。
next_mcl_call_id() -> u64
    生成调用序号：私有函数
    行为：静态 AtomicU64 从 1 起，Relaxed fetch_add(1)，命令和 turn 共用。
command_value_to_lua(value: crate::MclCommandValue) -> Result<LuaValue, LuaRuntimeError>
    转换命令Lua值：私有函数
    行为：Unit=Nil、Text=String、ResourceId inner=字符串 Array；消息经 mcl_message_to_json/json_to_lua；Paths 和 ResourceImport 输出与 JSON API 相同字段。
mcl_message_to_json(message: margatroid_types::MclMessage) -> Result<serde_json::Value, LuaRuntimeError>
    转换消息JSON：私有函数
    行为：输出 type=system/user/assistant/tool/error/inject 的对象和对应 content/reasoning/tool_calls/resource_id/tool_call_id/failed/message；Inject 递归转换内部消息（usage=None）；外层可选 usage 序列化后附加，失败 EnvironmentFailed。
json_to_lua(value: serde_json::Value) -> Result<LuaValue, LuaRuntimeError>
    转换JSON为Lua值：私有函数
    行为：递归转换 Null/Bool/Number/String/Array/Object；数字优先 i64 后 f64，不支持则 EnvironmentFailed。
lua_to_json_binding(value: LuaValue) -> Result<serde_json::Value, LuaRuntimeError>
    转换Lua绑定数据：私有函数
    行为：lua_to_json 的错误映射 InvalidRequest。
lua_to_json(value: LuaValue) -> Result<serde_json::Value, MclError>
    转换Lua值为JSON：私有函数
    行为：递归转换 Nil/Boolean/Integer/Number/String/Array/Object；非有限 f64 无法构造 JSON Number 时 TypeMismatch。
rejected_tool_resource(world: &World, agent: core_plugin::Entity, call: &ToolCall) -> ResourceRef
    取得拒绝调用的资源标识：私有函数
    行为：tool_by_name 查 entry.resource_id；失败尝试解析 tool_name；再失败构造 tool:builtin/invalid，仅作为拒绝消息身份占位。
begin_wake(world: &mut World, request: MclDomainRequest) -> Result<(), MclError>
    执行唤醒：私有函数
    行为：查 entity/Agent，要求 lifecycle=Creating/Running、lua.vm_id 及 LuaRuntimeHandle；按目标引用字符串 runtime.wake，失败 EffectInvalid；成功直接 reply.send(Unit)，不登记 pending。
begin_ready(world: &mut World, request: MclDomainRequest) -> Result<(), MclError>
    执行就绪通知：私有函数
    行为：要求 Agent/lua.vm_id；仅 Creating 且 initialization 未 complete 时发 AgentInitializationCompleted；直接 reply.send(Unit)，不登记 pending。
begin_inference(world: &mut World, request: MclDomainRequest, ref_block_id: String) -> Result<(), MclError>
    发起推理：私有函数
    行为：要求 entity、当前 turn、Agent、指定 RefBlock；恰好一个 Message RefMerge，至多一个 ResourceId RefMerge；select 动态消息/资源（无资源 merge 时空列表）；resolve_agent_tool_definitions 失败 ImportMissing；先写 inference.pending[(agent,turn)] 的 tool_schema，再登记 Inference state（vm_id=None、turn_id=Some(turn)、collected/expected_calls 为空）到 effects[turn_id]，键冲突/缺 Resource 为 EffectAlreadyPending；发 InferenceRequestEvent，消息只取 MclMessage.message。
    限制：inference.pending 更新先于 effects 冲突检查，不回滚；无资源 merge 时不退回 visible/default_visible 工具列表。
begin_import(world: &mut World, request: MclDomainRequest, resource_id: margatroid_types::ResourceRef, alias: String) -> Result<(), MclError>
    发起资源导入：私有函数
    行为：要求 Agent；引用须精确在 info.image_dependencies 中，tool:builtin/hook 按 type/scope/name 判断为例外（不额外检查 tag）；登记 mcl-import:<command_id> state，发 ToolRegisterRequest{alias:Some(alias)}；缺 pending Resource 为 ImportFailed。
    限制：所有资源均经 Provider，不在镜像目录读 prompt；insert 不检查同键事务。
imported_resource_definition(entry: &ResourceMapEntry) -> Result<resource_library_plugin::ResourceDefinition, MclError>
    构造导入资源定义：私有函数
    行为：要求 content；Tool/Skill/Hook 还要求 template，使用 description/parameters 和 script_path/content_path 构造对应定义；Prompt 使用 role/content；Sandbox 使用 policy；缺数据 ImportFailed。
```

## 调用与持有关系

```text
App.World -> PendingMclImports、PendingMclEffects、LuaRuntimeHandle。
LuaRuntimeHandle -> mcl provider（sender/runtime clone）-> 宿主函数。
命令事件 -> 审计/解析 -> 直接操作回执，或领域事件 -> 同步回执/等待 Provider、推理、工具响应。
等待响应 -> MclDomainResponse -> command_reply_system -> 原 oneshot（共享回执最多完成一次）。
Agent -> 实际 blocks/ref_blocks、state_bindings、资源映射、turn、inference/tools pending、memory。
```

# handler

## 函数

公开：

```text
parse_operation(command: &str, binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    解析命令：公开函数
    行为：
        任何分号都 InvalidCommand；normalize_command 后区分大小写分派 IMPORT/CREATE/GET/MERGE/REF_MERGE/INJECT/BIND/LOAD/EXPOSE/EMIT EFFECT，其他形状 InvalidCommand。
        IMPORT <ResourceRef> AS <alias>：严格四词，拒绝 binding，校验 alias，引用解析失败 InvalidResourceId。
        CREATE/GET/MERGE/REF_MERGE/BIND/LOAD 拒绝 binding；GET 严格两词。
        MERGE/REF_MERGE <inners...> FROM <block>：至少四词，inner 去尾逗号后 path 校验，仅产出 sources。
        BIND <block> TO STATE <name> / LOAD STATE <name> INTO <block>：严格五词并校验标识符。
        EXPOSE <domain>：严格两词，binding 必须 object（缺失/非对象均 BindingMissing）；校验 domain/field；值须 selector 字符串（否则 TypeMismatch），此时只检查语法。
execute_direct_operation(world: &mut World, request: &MclCommandRequest, operation: MclOperation) -> Result<MclCommandValue, MclError>
    执行直接操作：公开函数
    行为：
        查 entity；取得可变 Agent 前，把 resources.resources 中所有 prompt 引用从资源库收集 content（任一失败 TypeMismatch，即使命令不需要 prompt）；取得 Agent 后分派。
        CreateBlock：空声明构造 typed 空数组，Merge 声明 AgentMcl.merge 拷贝 sources 当前值；字段重复 InvalidCommand；create_block 的已有 ID 等错误统一 TypeMismatch。
        CreateRefBlock：AgentMcl.ref_merge 保存同类型普通字段路径；重复 merge_id 后者覆盖；create_ref_block 错误 TypeMismatch。
        Get：get_selector_value，支持普通字段和 RefMerge 实时展开值。
        BindState：bind_state 后立即序列化整个普通 block 并 memory.set_state，绑定失败 TypeMismatch、写失败 ImportMissing。
        LoadState：读 blob，缺失无操作；存在则反序列化 Block/merge_block，只覆盖当前已有同类型字段，未知字段忽略，空数组也覆盖；错误 TypeMismatch/ImportMissing。
        Expose：解析并实际选择每个 selector，保存 selector_to_string BTreeMap；替换该 domain 的 expose_mappings，再 refresh_exposed 全部 domain。
        Inject：读目标类型/长度；source selector 取快照，bindings 按目标类型转换并追加；map_selector 后 replace_range，普通 Block 才可写；记录 changed=目标路径。
        其他（包括顶层 Merge/RefMerge、Import/Emit）EffectInvalid。
        仅 changed 存在（INJECT）时，精确匹配 visible/default_visible_source 路径则更新对应集合；refresh_exposed 全部 domain；state_bindings 找首个绑定该 block 的 state，将完整 block 序列化写回 memory，最后返回值。
    限制：非事务，INJECT 后 expose/写 state 失败不恢复 block，EXPOSE 刷新失败不恢复映射；CREATE/LOAD 不刷新 exposed/visibility 或自动写 state。
history_append(world: &mut World, agent_id: &ResourceRef, message: margatroid_types::MclMessage, fallback_turn_id: &str, source: &str) -> Result<MclDomainValue, MclError>
    追加历史消息：公开函数
    行为：查 entity/Agent；当前 turn 优先，缺失用 fallback；Assistant 取当前 inference.pending tool_schema，其余为空；emit AgentHistoryMessageWriteRequested，传 message.message/usage/source，返回 Unit，不自动 INJECT/写 state。
history_record(world: &mut World, agent_id: &ResourceRef, kind: String, content: String, payload: String, source: &str) -> Result<MclDomainValue, MclError>
    追加历史记录：公开函数
    行为：只查 entity（不额外要求 Agent 组件），emit AgentHistoryRecordWriteRequested 携带原字符串/source，返回 Unit。
domain_to_command(value: MclDomainValue) -> MclCommandValue
    转换领域结果：公开函数
    行为：直接返回 value，命令值为领域值别名。
```

## 函数（解析与绑定）

私有：

```text
normalize_command(command: &str) -> String
    规范化命令文本：私有函数
    行为：折叠外部空白，选择器内部去紧邻 '['/逗号后的空白，保留数字/逗号，最后 trim；不是完整词法分析器。
parse_effect(words: &[&str], binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    解析效果指令：私有函数
    行为：
        缺 effect 名 EffectInvalid；wake/ready/start_turn/finish 严格三词且拒绝 binding；inference FROM <ref_block> 严格五词/校验 ID/拒绝 binding；visibility_source/default_visibility_source FROM <path> 须 All（无方括号）且拒绝 binding；history_append/history_record/tool_call/sandbox_use 须严格 FROM ?/required_binding；其他 EffectInvalid。
        history_append：parse_message。
        history_record：kind 非空字符串；content=JSON字符串 {command:cmd 或 null}，payload=JSON字符串 {args:arg 或 null}。
        tool_call：反序列化 Vec<ToolCall>，非空、id/tool_name/arguments 字符串非空、id 唯一，否则 ToolCallInvalid；不在此验证 arguments 是合法 JSON。
        sandbox_use：字符串为单 alias，字符串数组为多个，空 object 为零个（Lua 空表兼容），非空 object/其他类型 TypeMismatch。
parse_create(command: &str) -> Result<MclOperation, MclError>
    解析创建指令：私有函数
    行为：取首 '(' 与末 ')'，缺括号 ParseFailed；head 须三词、校验 block_id；BLOCK body 按空白解析 <inner> MESSAGE/RESOURCE 及 MERGE <inners...> FROM <block> AS <inner>，接受独立逗号/尾逗号；REF_BLOCK body 按 REF_MERGE 子串切段，解析 REF_MERGE <inners...> FROM <block> AS <merge>，仅校验 merge_id，source 路径此分支直接构造；返回声明，存在性/类型由执行检查。
    限制：不检查 ')' 后余文；REF_BLOCK 无 REF_MERGE 时可为空 merges、前缀可能忽略；普通重复字段执行时拒绝，引用重复字段覆盖，非统一解析拒绝。
parse_inject(words: &[&str], binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    解析注入指令：私有函数
    行为：首 TO 前须至少一个来源、后恰好一个目标；target 按 Inject 解析；source=[] 拒绝 binding/转空 JSON Array，source=? 要 binding；单来源含 '.' 按 Get selector 解析；其他多个去尾逗号的标识符转字符串 bindings，拒绝空文本、? 及 binding。
    限制：selector source 分支不 reject_binding，额外 binding 被忽略；完整引用可通过 ? 字符串 binding 传入，不作为裸 alias 标识符语法。
parse_endpoint(value: &str) -> Result<MclEndpoint, MclError>
    解析区间端点：私有函数
    行为：trim 后空=Open，否则解析 i64，失败 InvalidCommand。
parse_index(value: &str) -> Result<i64, MclError>
    解析索引：私有函数
    行为：trim 后解析 i64，失败 InvalidCommand。
parse_selector(value: &str, position: MclSelectorPosition) -> Result<MclSelector, MclError>
    解析选择器：私有函数
    行为：有 '[' 要求末字符 ']'；取首 '[' 前路径，按首 '.' 分 block/inner 并 path 校验；无括号为 All，括号空为 Empty，一个整数为 Single，两段逗号分隔为 Range（可 Open），更多段 InvalidCommand；返回 Raw，不检查符号/越界。
path(block_id: &str, inner_id: &str) -> Result<BlockPath, MclError>
    构造字段路径：私有函数
    行为：分别 validate_identifier 后复制为 BlockPath。
validate_identifier(value: &str) -> Result<(), MclError>
    验证标识符：私有函数
    行为：首字节 ASCII 字母/_，余字节只能 ASCII 字母数字/_/./-，拒绝空、.、..，失败 InvalidCommand。
required_binding<'a>(binding: Option<&'a serde_json::Value>) -> Result<&'a serde_json::Value, MclError>
    取得必需绑定：私有函数
    行为：None 返回 BindingMissing。
reject_binding(binding: Option<&serde_json::Value>) -> Result<(), MclError>
    拒绝额外绑定：私有函数
    行为：Some（包括 JSON null）InvalidCommand，否则成功。
parse_message(value: &serde_json::Value) -> Result<margatroid_types::MclMessage, MclError>
    解析消息绑定：私有函数
    行为：clone 后 message_from_lua_json。
message_from_lua_json(value: serde_json::Value) -> Result<margatroid_types::MclMessage, MclError>
    转换Lua消息JSON：私有函数
    行为：type 须字符串，支持 system/user/assistant/tool/error；system/user content 非字符串/缺失默认空；assistant reasoning/content 为可选字符串，tool_calls 数组反序列化（空 object/其他值为零个），usage 可选反序列化；tool resource_id 必需且反序列化 ResourceRef，tool_call_id/content 默认空、failed 默认 false；error message 默认空；除 Assistant 外 usage=None；未知 type（包括 inject）/转换失败 TypeMismatch。
empty_inner(kind: InnerType) -> BlockInner
    构造空字段：私有函数
    行为：Message/ResourceId 分别构造 typed 空 Vec。
binding_to_inner(value: &serde_json::Value, kind: InnerType, aliases: &std::collections::HashMap<String, ResourceRef>, sources: &std::collections::HashMap<ResourceRef, std::sync::Arc<str>>) -> Result<BlockInner, MclError>
    转换绑定为字段值：私有函数
    行为：Message 字符串须已导入 alias 且 sources 有 prompt 内容，scope=system 构造 System，其余 scope 一律 User（不读取 PromptResource.role），缺 alias/content ImportMissing；Message 数组逐项 message_from_lua_json，非数组 parse_message；ResourceId 字符串优先 aliases 再完整 ResourceRef 解析（失败 ImportMissing），数组直接反序列化 Vec<ResourceRef>、元素不展开 alias，其他 TypeMismatch。
```

## 函数（选择与刷新）

私有：

```text
map_selector(selector: &MclSelector, length: usize) -> Result<MclSelector, MclError>
    映射选择器：私有函数
    行为：All -> Standard[0,len]；Standard 原样 clone（不重新校验）；Raw Empty -> [0,0]，Single 调 map_single，Range 调 map_endpoints；保留路径。
map_single(position: MclSelectorPosition, index: i64, length: usize) -> Result<(usize, usize), MclError>
    映射单索引：私有函数
    行为：空字段 Get[0] 特例 [0,0]；offset=len+1；非负 Get=[x,x+1]，非负 Inject=[x,x]，负 Get=[offset+x-1,offset+x]，负 Inject=[offset+x,offset+x]；交 select_range。
map_endpoints(start: MclEndpoint, end: MclEndpoint, length: usize) -> Result<(usize, usize), MclError>
    映射区间端点：私有函数
    行为：双 Open InvalidCommand；左 Open 起点=0，右 Open 终点=len；显式非负端点原样，负端点加 len+1；两个显式端点混号 InvalidCommand；交 select_range。
select_range(start: i64, end: i64, length: usize) -> Result<(usize, usize), MclError>
    验证标准区间：私有函数
    行为：len=0 仅 [0,0] 有效，其他要求 0<=start<=end<=len，失败 TypeMismatch，成功转 usize。
selector_path(selector: &MclSelector) -> BlockPath
    取得选择器路径：私有函数
    行为：从任何变体 clone path。
get_selector_value(mcl: &agent_plugin::AgentMcl, selector: &MclSelector) -> Result<MclDomainValue, MclError>
    读取选择器结果值：私有函数
    行为：select 取长度、map_selector、select_standard；原 Raw Single 时 Message 返回单个 Message，ResourceId 返回单元素 inner，空结果 Unit；其他返回 Inner。
select_selector(mcl: &agent_plugin::AgentMcl, selector: &MclSelector) -> Result<BlockInner, MclError>
    选择字段区间：私有函数
    行为：select 取长度后 map_selector/select_standard，始终返回 typed inner。
select_standard(mcl: &agent_plugin::AgentMcl, selector: &MclSelector) -> Result<BlockInner, MclError>
    执行标准选择：私有函数
    行为：只接受 Standard，按 path select 后 slice_inner；AgentMcl 错误 TypeMismatch。
slice_inner(values: &BlockInner, start: usize, end: usize) -> Result<BlockInner, MclError>
    切片字段值：私有函数
    行为：左闭右开 get(start..end) 并 clone，保持变体；越界 TypeMismatch。
append_inner(target: &mut BlockInner, values: BlockInner) -> Result<(), MclError>
    追加字段值：私有函数
    行为：同变体 extend，不同变体 TypeMismatch。
selector_to_string(selector: &MclSelector) -> String
    序列化选择器文本：私有函数
    行为：输出 block.inner、block.inner[]、[x] 或 [start,end]；Open 输出空，Standard 输出非负双端，不映射 Raw 数值。
refresh_exposed(resources: &mut agent_plugin::AgentResourceMap, mcl: &agent_plugin::AgentMcl) -> Result<(), MclError>
    刷新公开映射：私有函数
    行为：遍历全部 expose_mappings，以 Get 解析/读取并 command_value_to_json；构造嵌套 BTreeMap，全部成功才整体替换 resources.exposed。
```

## 选择器、合并和 STATE 语义

支持 `b.f`、`b.f[]`、`b.f[x]`、`b.f[x,y]`、`b.f[,y]`、`b.f[x,]`。最终标准区间左闭右开；正双端原样，负双端都加 len+1，可理解为负索引左开右闭。例如 len=4：GET[-1] 取最后一个，GET[-3,-1] 映射 [2,4]；INJECT[-1] 为尾部插入，INJECT[0] 为头部插入。`[,]`、显式混号区间 InvalidCommand，逆序/越界 TypeMismatch；空字段 GET[0] 为 Unit。`[]` 目标是 [0,0] 插入，不是清空整字段；`INJECT [] TO b.f`才清空整字段。

普通 Block 可读写；RefBlock 的合并字段是只读实时视图，可作 INJECT 来源、不可作目标。CREATE BLOCK 内 MERGE 拷贝值，可读取 AgentMcl.select 支持的字段；CREATE REF_BLOCK 内 REF_MERGE 只保存普通 Block 字段路径，不可引用 RefBlock。顶层 MERGE/REF_MERGE 虽解析成功，实际执行 EffectInvalid，当前不返回 Paths。

```text
BIND <block> TO STATE <name>
    行为：普通 block 必须存在；同 state 绑定不同 block 失败，同一对幂等；本命令立即写完整 block。一个 block 可以绑定多个 state，但 INJECT 自动写回只找 HashMap 中首个匹配，顺序不稳定。
LOAD STATE <name> INTO <block>
    行为：blob 不存在保留默认值（此路径不强制验证 block 存在）；存在则覆盖已有同类型字段，未知字段忽略、空数组也覆盖；不自动建立绑定。
INJECT 已绑定普通 block
    行为：修改后写回完整 block 到首个匹配 state；不是“所有修改都会持久化”，LOAD/CREATE 无 changed 标记。
```

history 事件与 state blob 是独立通道，不自动同步。Driver 的 Lua source/origin/hash 和 VM 由外部插件管理；本插件没有程序包装、程序资源 ID、compile_mcl 或 mailbox 解析函数。

# events

## 类型

公开：

以下结构体均 Clone + Debug；所有列出字段均 pub，事务状态不是 Event。

```text
MclCommandRequest：公开事件
    id: MclCommandId--命令 ID
    source: String--调用方来源标识
    agent_id: ResourceRef--目标 agent
    command: String--原文
    binding: Option<serde_json::Value>
    reply: MclCommandReply
    impl Event for MclCommandRequest
MclDomainRequest：公开事件
    id: MclCommandId
    source: String--保留来源，history 使用
    agent_id: ResourceRef
    operation: MclOperation
    reply: MclCommandReply
    impl Event for MclDomainRequest
MclDomainResponse：公开事件
    id: MclCommandId
    agent_id: ResourceRef
    result: Result<crate::MclDomainValue, MclError>
    reply: MclCommandReply
    impl Event for MclDomainResponse
MclImportState：公开结构体
    command_id: MclCommandId
    agent_id: ResourceRef
    agent: Entity
    resource_id: ResourceRef
    alias: String
    reply: MclCommandReply
MclEffectState：公开结构体
    command_id: MclCommandId
    agent_id: ResourceRef
    agent: Entity
    vm_id: Option<margatroid_types::LuaVmId>--当前发起路径 None，响应不校验
    kind: crate::MclPendingEffectKind
    collected: Vec<margatroid_types::Message>
    expected_calls: Vec<String>--调用 ID 原始顺序
    turn_id: Option<String>
    reply: MclCommandReply
```

# types

## 公开重导出

```text
pub use agent_plugin::AgentMcl;
pub use margatroid_types::{Block, BlockAssembly, BlockInner, BlockPath, InnerType,
    MclMessage, RefBlock, RefBlockAssembly, RefMerge};
```

这些类型在其他 crate 定义；本文件没有 MCL 程序加载函数。以下为本地全部类型。

## 类型

公开：

```text
ResourceImportReceipt：公开结构体，Clone + Debug + PartialEq + Eq
    resource_id: ResourceRef--公开字段
    alias: String--公开字段
    available: bool--公开字段
    error: Option<String>--公开字段
MclCommandId(String)：命令标识，公开元组结构体，String 私有，Clone + Debug + PartialEq + Eq
    new(value: impl Into<String>) -> Result<Self, crate::MclError>
        构造命令标识：公开关联函数
        行为：只拒绝空字符串为 InvalidCommand，不检查唯一性/trim。
    as_str(&self) -> &str
        借用命令标识：公开方法
        行为：借用内部字符串。
MclCommandReply：公开结构体，Clone + Debug
    sender: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<MclCommandValue, crate::MclError>>>>>--私有
    new(sender: tokio::sync::oneshot::Sender<Result<MclCommandValue, crate::MclError>>) -> Self
        构造命令回执：公开关联函数
        行为：Arc/Mutex/Some 包装 sender。
    send(&self, result: Result<MclCommandValue, crate::MclError>)
        发送命令回执：公开方法
        行为：锁成功则 take sender 并发送，最多一次；忽略接收端关闭和 poisoned 锁。
MclBinding(serde_json::Value)：绑定值，公开元组结构体，内部字段公开，Clone + Debug
MclSelector：公开枚举，Clone + Debug + PartialEq + Eq
    All(BlockPath)
    Raw { path: BlockPath, range: MclRange, position: MclSelectorPosition }
    Standard { path: BlockPath, start: usize, end: usize }
MclRange：公开枚举，Clone + Copy + Debug + PartialEq + Eq
    Empty
    Single(i64)
    Range(MclEndpoint, MclEndpoint)
MclSelectorPosition：公开枚举，Clone + Copy + Debug + PartialEq + Eq
    Get
    Inject
MclEndpoint：公开枚举，Clone + Copy + Debug + PartialEq + Eq
    Value(i64)
    Open
MclInjectSource：公开枚举，Clone + Debug
    Selector(MclSelector)
    Bindings(Vec<MclBinding>)
BlockFieldDeclaration：公开枚举，Clone + Debug
    Empty { inner_id: String, inner_type: InnerType }
    Merge { inner_id: String, sources: Vec<BlockPath> }
RefMergeDeclaration：公开结构体，Clone + Debug
    merge_id: String--公开字段
    sources: Vec<BlockPath>--公开字段
MclEffectCommand：公开枚举，Clone + Debug
    Wake
    Ready
    StartTurn
    Inference { ref_block_id: String }
    ToolCall { calls: Vec<ToolCall> }
    Finish
    HistoryAppend { message: MclMessage }
    HistoryRecord { kind: String, content: String, payload: String }
    SandboxUse { aliases: Vec<String> }
    VisibilitySource { source: BlockPath }
    DefaultVisibilitySource { source: BlockPath }
MclOperation：公开枚举，Clone + Debug
    CreateBlock { block_id: String, fields: Vec<BlockFieldDeclaration> }
    CreateRefBlock { block_id: String, merges: Vec<RefMergeDeclaration> }
    Merge { sources: Vec<BlockPath> }
    RefMerge { sources: Vec<BlockPath> }
    Import { resource_id: ResourceRef, alias: String }
    Get { selector: MclSelector }
    Inject { source: MclInjectSource, target: MclSelector }
    BindState { block_id: String, state_name: String }
    LoadState { state_name: String, block_id: String }
    Expose { domain: String, mappings: Vec<(String, String)> }
    Emit { effect: MclEffectCommand }
MclDomainValue：公开枚举，Clone + Debug
    Unit
    Inner(BlockInner)
    Paths(Vec<BlockPath>)
    Message(MclMessage)
    ResourceImport(ResourceImportReceipt)
    Text(String)
MclCommandValue = MclDomainValue：命令值，公开类型别名
MclPendingEffectKind：公开枚举，Clone + Debug + PartialEq + Eq
    Inference
    ToolCall { expected: usize }
```

ToolCall 是 effect 参数，不是 Block inner。Paths 保留公开协议/转换分支，但当前操作没有产生它的成功执行路径；Text 由兼容 CapturedInferenceResponse 分支产生，普通 inference 返回 Message。

# error

## 类型

公开：

```text
MclError：公开枚举，Clone + Debug + PartialEq + Eq
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
    EffectFailed(String)
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
    impl fmt::Display for MclError
        fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：公开trait方法
            行为：write!(f, "{self:?}")，按 Debug 显示。
    impl std::error::Error for MclError--无自定义方法
    impl From<resource_id_plugin::ResourceIdLookupError> for MclError
        from(error: resource_id_plugin::ResourceIdLookupError) -> Self
            转换查询错误：公开trait方法
            行为：Missing -> AgentMissing，Duplicate -> DuplicateAgent，PluginMissing -> AgentRuntimeMissing。
```

## 使用边界与覆盖

六文件非测试逻辑没有构造 `InnerMissing`、`MergeMissing`、`MailboxFailed`、`InferenceFailed`、`SourceReadFailed`、`SourceTooLarge`、`SourceInvalidUtf8`、`ImportCycle`、`InvalidProgramKind`。保留错误变体不证明对应功能存在。大量 AgentMcl 错误折叠为 TypeMismatch，Provider 错误折叠为 ImportFailed；EffectFailed 保留外部字符串。BlockMissing/RefBlockMissing 仅少数明确读取处保留字段，不是所有缺字段都有精细分类。

完整阅读范围：lib 17、system 1625、handler 1185、events 57、types 202、error 80 行（含测试以识别边界）；本文覆盖本地全部非测试类型、字段、变体、方法、trait 实现与自由函数。另核对 AgentMcl 的 merge/ref_merge/replace_range/bind_state/merge_block 委托语义。外部 Provider/组件/资源库内部实现不在六文件内展开；这里只陈述实际调用，不承诺端到端事务或资源生命周期保证。
