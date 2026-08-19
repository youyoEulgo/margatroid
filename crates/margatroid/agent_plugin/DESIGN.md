# AgentPlugin

## 类型

公开：
```text
AgentPlugin：Agent实例与消息循环插件，公开结构体--安装创建、消息处理和MCL领域事件适配System
    schedule: String--System所属Schedule，私有
    new() -> Self
    with_schedule(mut self, schedule: impl Into<String>) -> Self
    impl Default for AgentPlugin
    impl Plugin for AgentPlugin
        build(self, app: &mut App)
            构建插件：要求RuntimePlugin、ToolPlugin、MclPlugin和目标Schedule存在，插入PendingVisibilityCommands，挂载Agent创建、Driver状态、可见性命令回执和消息处理System

AgentCreateRequest：Agent创建请求，公开事件--WorkspacePlugin交付Agent自有字段
    id: String--Workspace创建子请求ID
    agent_id: ResourceId--稳定Agent资源ID，格式agent:<workspace>/<name>:latest
    workspace_id: Entity--所属Workspace Entity
    base_driver: MclDriverSource--AgentImage根目录base.lua验证后的内禀Base Driver，身份继承Agent资源ID
    tool_environment: AgentToolEnvironment--项目根、镜像根和主目录解析环境，由ToolPlugin定义类型
    ordered_messages: Vec<Message>--按实际发生顺序恢复的User、Assistant与Tool消息，作为MCL恢复输入
    token_usage: TokenUsage--从历史Assistant行恢复的累计Token
    last_input_tokens: u64--从历史最后一条Assistant恢复的输入Token
    context_window_tokens: u64--InferencePlugin标准化后的模型总上下文窗口，交给Base Driver读取
    impl Event for AgentCreateRequest

AgentCreateResult：Agent Entity创建结果，公开事件--无论成功失败都恰好发送一次
    id: String--原创建子请求ID
    agent_id: ResourceId--原请求中的稳定Agent资源ID
    result: Result<Entity, AgentCreateError>--成功时为已经挂载Agent自有组件和空AgentResourceMap的Entity
    impl Event for AgentCreateResult

AgentCreated：Agent Entity创建成功通知，公开事件--Agent自有组件、AgentResourceMap与AgentMcl已经建立，Base Driver初始化完成并进入start等待
    id: String--原创建子请求ID
    agent_id: ResourceId--稳定Agent资源ID
    agent: Entity--新建Agent Entity
    impl Event for AgentCreated

InjectAgentVisibleResource：注入单项动态可见资源，公开事件
    id: String--操作通知ID
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--待注入的完整资源ID
    impl Event for InjectAgentVisibleResource

RemoveAgentVisibleResource：删除单项动态可见资源，公开事件
    id: String--操作通知ID
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--待删除的完整资源ID
    impl Event for RemoveAgentVisibleResource

SetAgentDefaultResourceVisibility：设置单项默认资源可见性，公开事件--供外部用户操作，AgentPlugin校验资源必须属于默认可见性
    id: String--操作通知ID
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--待开关的完整资源ID
    visible: bool--true注入，false删除
    impl Event for SetAgentDefaultResourceVisibility

RestoreAgentDefaultVisibility：显式恢复默认可见性，公开事件--清除manual修改后重新用Base Driver定义的tool_default覆盖tool_dynamic；Agent创建由base.lua初始INJECT完成
    id: String--操作通知ID，也是各默认资源通知的父ID
    agent: Entity--目标Agent Entity
    impl Event for RestoreAgentDefaultVisibility

RemoveAllAgentVisibleResources：删除全部动态可见资源，公开事件
    id: String--操作通知ID，也是各删除通知的父ID
    agent: Entity--目标Agent Entity
    impl Event for RemoveAllAgentVisibleResources

AgentVisibleResourceInjected：资源注入成功通知，公开事件--只表示该资源当前已经进入动态可见性
    id: String--触发本次注入的操作通知ID
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--已经可见的完整资源ID
    impl Event for AgentVisibleResourceInjected

AgentVisibleResourceRemoved：资源删除通知，公开事件--单项删除请求幂等成功时也发送；批量删除只为实际可见资源逐项发送
    id: String--触发本次删除的操作通知ID
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--已经不可见的完整资源ID
    impl Event for AgentVisibleResourceRemoved

AgentVisibleResourceInjectionFailed：资源注入失败通知，公开事件--不退出Agent或Workspace
    id: String--触发本次注入的操作通知ID
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--注入失败的完整资源ID
    error: AgentVisibilityError--稳定错误
    impl Event for AgentVisibleResourceInjectionFailed

AgentCreateErrorKind：Agent创建错误分类，公开枚举
    InvalidRequest
    DuplicateAgent
    WorkspaceMissing
    ContextInvalid
    ResourceMapSetupFailed
    MclSetupFailed

AgentCreateError：Agent创建错误，公开结构体--不包含上下文正文
    kind: AgentCreateErrorKind
    message: String
    kind(&self) -> AgentCreateErrorKind
    message(&self) -> &str

AgentVisibilityErrorKind：可见性错误分类，公开枚举
    AgentMissing
    VisibilityMissing
    ResourceMapMissing
    RegistrationFailed
    RegistrationResponseMismatch

AgentVisibilityError：可见性错误，公开结构体--稳定描述Agent、注册和回执错误，不包含资源正文
    kind: AgentVisibilityErrorKind
    message: String
    kind(&self) -> AgentVisibilityErrorKind
    message(&self) -> &str

AgentIdentity：Agent稳定身份，公开Component
    id: ResourceId--type=agent的唯一资源ID，私有
    id(&self) -> &ResourceId
    impl Component for AgentIdentity

AgentWorkspaceId：Agent所属Workspace，公开Component
    workspace_id: Entity--Workspace Entity，私有
    workspace_id(&self) -> Entity
    impl Component for AgentWorkspaceId

Agent上下文不再由AgentPlugin维护第二份AgentContext组件。消息数组、pending_tool和工具可见性
统一存放在AgentMcl；MemoryPlugin的实时上下文更新由MCL事务提交后的上下文变更事件驱动。

AgentTokenUsage：Agent累计Token状态，公开只读Component
    total_input_tokens: u64--历史Assistant响应累计输入Token
    total_output_tokens: u64--历史Assistant响应累计输出Token
    total_cache_hit_tokens: u64--历史Assistant响应累计缓存命中Token
    cache_hit_rate: f64--total_cache_hit_tokens / total_input_tokens；总输入为0时为0
    last_input_tokens: u64--最近一条普通Assistant响应的输入Token；启动时从历史最后一条Assistant恢复
    context_window_tokens: u64--当前Agent模型配置的最大上下文窗口
    total_input_tokens(&self) -> u64
    total_output_tokens(&self) -> u64
    total_cache_hit_tokens(&self) -> u64
    cache_hit_rate(&self) -> f64
    last_input_tokens(&self) -> u64
    context_window_tokens(&self) -> u64
    add(&mut self, usage: &TokenUsage)
        累加用量：crate公开方法，三项使用饱和加法，覆盖last_input_tokens，并重新计算cache_hit_rate
    impl Component for AgentTokenUsage

AgentPluginInstalled：安装标记，公开Resource

AbortAgentTurn：中止Agent当前轮次，公开事件
    id: String--API请求ID
    agent: Entity--目标Agent
    impl Event for AbortAgentTurn

AgentContextCompactRequest：Agent实时上下文压缩请求，公开事件--只定义压缩机制，不拥有触发策略
    id: String--压缩请求ID
    agent: Entity--目标Agent Entity
    retain_messages: usize--原样保留的末尾长期消息数量；其余头部消息进入摘要
    impl Event for AgentContextCompactRequest

WorldAgentExt：World Agent扩展，公开trait
    agent(&self, id: &ResourceId) -> Option<Entity>
        按身份查询：公开方法，返回稳定资源ID匹配且仍存活的Agent Entity
    agent_is_working(&self, agent: Entity) -> Option<bool>
        查询工作状态：公开只读方法，AgentStatus存在时返回当前是否有未结束普通交互或上下文压缩
    inject_agent_visible_resource(&self, id: impl Into<String>, agent: Entity, resource_id: ResourceId)
        注入可见资源：公开方法，发送InjectAgentVisibleResource并唤醒Runtime
    remove_agent_visible_resource(&self, id: impl Into<String>, agent: Entity, resource_id: ResourceId)
        删除可见资源：公开方法，发送RemoveAgentVisibleResource并唤醒Runtime
    restore_agent_default_visibility(&self, id: impl Into<String>, agent: Entity)
        恢复默认可见性：公开方法，发送RestoreAgentDefaultVisibility并唤醒Runtime
    remove_all_agent_visible_resources(&self, id: impl Into<String>, agent: Entity)
        删除全部可见资源：公开方法，发送RemoveAllAgentVisibleResources并唤醒Runtime
    impl WorldAgentExt for World
```

crate公开：
```text
AgentStatus：Agent工作占用状态，crate公开Component--不保存pending tool或压缩快照
    turn_id: Option<String>--当前普通交互轮次ID或上下文压缩请求ID；空表示Agent空闲
    begin_turn(&mut self, turn_id: String) -> Result<(), AgentStepError>
        开始工作：拒绝普通轮次与上下文压缩相互重叠
    finish_turn(&mut self, turn_id: &str) -> Result<(), AgentStepError>
        完成轮次：只允许完成当前turn
    abort_turn(&mut self) -> Option<String>
        中止轮次：清空并返回当前turn_id；空闲时返回None
    is_working(&self) -> bool
        查询工作状态：当前turn_id非空时返回true
    impl Component for AgentStatus
```

私有：
```text
AvailableTools：一次推理的临时工具规格集合，私有结构体
    definitions: Vec<ToolDefinition>--从当前AgentResourceMap取得的内部ToolSpec

PendingInferenceToolSchemas：飞行中推理的ToolSpec快照，私有Resource
    schemas: HashMap<(Entity, String), Vec<ToolDefinition>>--按Agent与turn_id关联下一条Assistant

PendingContextCompactions：飞行中的上下文压缩，私有Resource
    requests: HashMap<(Entity, String), PendingContextCompaction>--按Agent和压缩请求ID关联原始上下文快照

PendingContextCompaction：单次压缩快照，私有结构体
    original_messages: Vec<Message>--开始摘要前的完整长期消息，用于完成时校验
    retained_messages: Vec<Message>--不进入摘要、完成后仍原样位于摘要检查点之后的近期消息

ConversationTurnResult：单条消息处理结果，私有枚举
    WaitForTools--已发送ToolCallEvent，等待ToolPlugin完成批次
    FinishTurn--Assistant无工具调用，本轮结束
    RequestInference--上下文完整，可发送InferenceRequestEvent

AgentStepError：Agent处理错误，私有枚举
    AgentMissing
    IdentityMissing
    MclContextMissing
    StatusMissing
    TokenUsageMissing
    ResourceMapMissing
    InvalidMessage
    InvalidToolBatch
    ContextNotCompactable
    ContextChanged
    InvalidCompactionResponse
    Inference(InferenceError)
    Tool(ToolError)
    failure_message(&self) -> String
        构造稳定有界错误描述，不包含消息正文、工具参数或资源正文

PendingVisibilityCommands：等待MCL命令回执的外部可见性操作，私有Resource
    commands: HashMap<String, PendingVisibilityCommand>--MCL命令ID到原API操作、当前阶段和通知信息

PendingVisibilityCommand：单次外部可见性操作，私有结构体
    request_id: String
    agent: Entity
    resource_id: Option<ResourceId>
    action: Inject | Remove | SetDefault | Restore | RemoveAll
    phase: Import | Mutate
```

## 函数

```text
agent_create_system(world: &mut World)
    创建Agent：私有System，读取AgentCreateRequest
    行为：
        验证请求ID、agent_id、Workspace Entity及恢复消息结构；失败时发送AgentCreateResult::Err
        创建Entity并挂载AgentIdentity、AgentWorkspaceId、AgentToolEnvironment、由请求累计值构造的AgentTokenUsage和空AgentStatus
        调用ToolPlugin公开函数挂载空AgentResourceMap；失败时despawn已创建Entity并发送AgentCreateResult::Err
        调用MclPlugin为Entity挂载AgentMcl并异步启动base.lua；保存创建请求与临时Entity，暂不发送成功结果
        Base Driver的IMPORT通过MCL事件异步解析资源；此时不直接注册资源

agent_mcl_driver_state_system(world: &mut World)
    收集Base Driver初始化结果：私有System，读取MclDriverReady与MclDriverFailed
    行为：
        Ready必须匹配等待中的Agent创建；随后发送AgentCreated和AgentCreateResult::Ok(agent)
        Failed必须匹配等待中的Agent创建；despawn临时Entity并发送AgentCreateResult::Err(MclSetupFailed)
        每个AgentCreateRequest恰好产生一次最终结果，迟到或重复Driver通知记录稳定错误并忽略

agent_visibility_change_system(world: &mut World)
    把外部可见性操作转换为MCL命令：私有System
    行为：
        Inject先查AgentResourceMap；已存在时发送MCL事务追加到tool.tool_dynamic，不存在时先发送IMPORT命令，成功后再追加
        Remove发送MCL事务按resource_id从tool.tool_dynamic删除；不存在按幂等成功处理
        SetDefault先查询tool.tool_default，成员不存在则失败；随后按visible追加或删除tool.tool_dynamic
        Restore发送MCL命令用tool.tool_default COVER tool.tool_dynamic
        RemoveAll发送MCL命令清空tool.tool_dynamic
        每个操作生成内部MCL命令ID并写入PendingVisibilityCommands；不直接修改AgentMcl字段

collect_visibility_command_response_system(world: &mut World)
    收集MCL命令回执：私有System，读取MclCommandResponse并匹配PendingVisibilityCommands
    行为：成功后发送AgentVisibleResourceInjected或AgentVisibleResourceRemoved；失败发送AgentVisibleResourceInjectionFailed
        任一失败不结束Agent或Workspace；迟到回执只记录稳定错误

cleanup_dead_agent_state_system(world: &mut World)
    清理死亡Agent临时状态：删除PendingVisibilityCommands、PendingInferenceToolSchemas与PendingContextCompactions中对应项

agent_message_system(world: &mut World)
    处理Agent消息：私有System，读取AgentMessage并逐条调用handle_agent_message
    行为：失败时发送AgentFailure { kind: Agent }，不伪造Assistant或Tool消息

abort_agent_turn_system(world: &mut World)
    中止当前轮次：私有System，读取AbortAgentTurn
    行为：取得并清空AgentStatus当前turn；取消对应PendingInferenceToolSchemas与PendingContextCompactions；发送CancelInferenceRequest与CancelToolTurn；MCL中断消息序列的修复策略另行设计，当前不自动清空conversation或pending_tool；空闲Agent只记录警告

context_compaction_system(world: &mut World)
    处理上下文压缩：私有System，同时读取AgentContextCompactRequest与ContextCompactionInferenceResponse
    行为：请求逐条调用begin_context_compaction；响应逐条调用complete_context_compaction；失败时发送AgentFailure，不生成AgentMessage

begin_context_compaction(world: &mut World, request: &AgentContextCompactRequest, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    开始上下文压缩：私有函数
    行为：
        要求Agent存活、AgentStatus空闲、msg.pending_tool为空且msg.conversation数量大于retain_messages
        调用AgentStatus.begin_turn占用工作状态，阻止压缩期间开始普通对话轮次
        按conversation.len() - retain_messages切分待压缩头部和原样保留尾部
        保存完整original_messages和retained_messages到PendingContextCompactions
        构造System、待压缩头部消息和末尾压缩提示词，不携带工具规格
        发送ContextCompactionInferenceRequest；不修改AgentMcl，不写历史

complete_context_compaction(world: &mut World, response: &ContextCompactionInferenceResponse, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    完成上下文压缩：私有函数
    行为：
        取得并删除同Agent同请求ID的PendingContextCompaction，要求AgentStatus当前turn等于请求ID
        推理失败时结束占用并返回Inference错误；成功摘要必须非空
        要求当前msg.conversation仍等于original_messages且msg.pending_tool仍为空；不一致时结束占用并返回ContextChanged
        把摘要包装成带compacted-summary标记的User消息，后接retained_messages
        通过MCL原子事务用摘要消息和retained_messages覆盖msg.conversation，并发送上下文更新事件
        调用AgentStatus.finish_turn释放工作状态；不写历史，不生成普通AgentMessage

handle_agent_message(world: &mut World, event: &AgentMessage, events: &RuntimeEventSender) -> Result<ConversationTurnResult, AgentStepError>
    处理消息：私有函数
    行为：
        System返回InvalidMessage
        User开始或确认当前turn，写历史并送入Base Driver邮箱
        User只追加正文并发送InferenceRequestEvent；工具调用只接受Assistant.tool_calls
        Assistant先取得该turn对应的PendingInferenceToolSchemas；event.usage存在时先累加AgentTokenUsage；写入带usage的历史并送入Base Driver邮箱
        Assistant.tool_calls为空时结束当前turn
        Assistant.tool_calls非空时逐项检查tool_name存在于本次ToolSpec且对应ResourceMapEntry仍在tool.tool_dynamic；不满足时直接发送Message::Tool拒绝响应，提示模型检查当前ToolSpec，不发送ToolCallEvent
        Assistant所有调用均被拒绝时仍将拒绝响应作为Message::Tool交给MCL；MCL按tool_call_id清理pending_tool
        Assistant鉴权通过的tool_calls统一派发
        Tool写历史并送入Base Driver邮箱；Driver追加conversation、删除对应pending_tool，并在数组为空时请求下一次推理

take_pending_tool_schema(world: &mut World, agent: Entity, turn_id: &str) -> Vec<ToolDefinition>
    取得推理工具规格：私有函数，从PendingInferenceToolSchemas移除并返回当前Agent与turn对应的ToolSpec；不存在时返回空数组

record_history_message(world: &mut World, event: &AgentMessage, events: &RuntimeEventSender, tool_schema: Vec<ToolDefinition>)
    请求历史写入：私有函数
    行为：User原样发送且tool_schema和usage为空；Assistant原样发送并携带传入的ToolSpec与event.usage；Skill类型Tool保留resource_id和tool_call_id并把content替换为resource_id.to_string()；非Skill类型Tool原样发送；Tool的tool_schema和usage为空
    限制：Skill正文进入MCL conversation但不进入历史事件；非Skill工具响应正文完整写入历史事件

append_conversation_message(world: &mut World, agent: Entity, message: Message, events: &RuntimeEventSender) -> Result<(), AgentStepError>
build_available_tools(world: &World, agent: Entity) -> Result<AvailableTools, AgentStepError>
    构造工具规格：私有函数
    行为：
        按tool.tool_dynamic数组顺序读取ResourceMapEntry
        每个元素的tool_id和template必须同时为Some，否则失败
        克隆template并令name等于resource_name形成内部ToolSpec；Provider格式转换由InferencePlugin负责

dispatch_tool_calls(world: &World, turn_id: &str, agent: Entity, explicit: &[ToolCall], events: &RuntimeEventSender) -> Result<ConversationTurnResult, AgentStepError>
    派发工具：私有函数
    行为：
        验证显式调用ID和tool_name非空，且tool_name存在于当前AgentResourceMap
        验证同批ToolCall.id唯一
        为每个调用发送ToolCallEvent { turn_id, agent, call }
        没有调用时返回RequestInference，否则返回WaitForTools

dispatch_assistant_tool_calls(world: &mut World, turn_id: &str, agent: Entity, explicit: &[ToolCall], tool_schema: &[ToolDefinition], events: &RuntimeEventSender) -> Result<ConversationTurnResult, AgentStepError>
    校验并派发模型调用：私有函数
    行为：ResourceMapEntry无法解析或调用ID重复时返回InvalidToolBatch；ToolSpec中没有tool_name或映射资源不在动态可见性时发送Message::Tool { content: TOOL_PERMISSION_DENIED }；通过校验的调用交给dispatch_tool_calls

build_inference_context(world: &World, agent: Entity) -> Result<Vec<Message>, AgentStepError>
    组装上下文：交给AgentMcl按Block中的类型化有序数组生成；只展开请求定义选择的
    Message数组，pending_tool等执行状态数组不得进入模型请求

send_inference_request(world: &mut World, turn_id: &str, agent: Entity, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    发起推理：构造当前内部ToolSpec与完整上下文；把ToolSpec按(agent, turn_id)写入PendingInferenceToolSchemas后发送InferenceRequestEvent
    限制：只读取调用时刻的tool.tool_dynamic；正在IMPORT或解析失败的资源不进入本次请求
```

## 逻辑

```text
创建与可见性：
    AgentCreateRequest
        -> 创建Agent自有组件和空AgentResourceMap
        -> 挂载AgentMcl并启动base.lua
        -> IMPORT逐项解析AgentResourceMap；单项失败记录Unavailable但不终止Driver
        -> CREATE建立标准Block和Request
        -> 初始INJECT建立tool_default与tool_dynamic
        -> MclDriverReady（允许AgentResourceMap为空或部分可用）
        -> AgentCreated + AgentCreateResult

User：
    AgentMessage::User
        -> begin_turn
        -> 记录历史
        -> 作为MclRuntimeMessage进入Base Driver邮箱
        -> base.lua的start返回User消息
        -> Driver将完整User消息追加到conversation并提交InferenceRequestEvent

Assistant：
    AgentMessage::Assistant
        -> 记录历史
        -> 作为MclRuntimeMessage进入Base Driver邮箱
        -> base.lua的start返回Assistant消息
        -> Driver将完整Assistant消息追加到conversation数组
        -> 无tool_calls：提交finish
        -> 有tool_calls：逐个追加到pending_tool并提交tool_call Effect

Tool：
    AgentMessage::Tool { resource_id, tool_call_id, content }
        -> 记录历史
        -> Base Driver将完整Tool消息追加到conversation数组
        -> 按tool_call_id删除pending_tool数组中的对应ToolCall
        -> pending_tool非空：继续等待其余Tool响应
        -> pending_tool为空：MCL产生InferenceRequestEvent
```

## 边界

```text
AgentPlugin负责Agent创建、当前turn、MCL领域事件适配、内部ToolSpec构造和推理调度。
AgentPlugin消费MclBlockingInferenceRequest，将指定MCL Request Block脱壳为领域消息后发起
ContextCompactionInferenceRequest；响应摘要通过原MCL命令回执返回，不产生AgentMessage。
AgentPlugin负责维护AgentTokenUsage；只有进入普通Assistant消息链路的Provider usage会累加，压缩推理不计入。
AgentPlugin把外部可见性操作转换为MCL命令，不直接修改AgentMcl，不维护第二套可见性Component。
WorkspacePlugin只在收到AgentCreateResult后挂载其余外部运行组件，不介入资源注册或可见性修改。
AgentPlugin不额外保存pending tool；pending_tool是AgentMcl中的类型化数组。AgentPlugin不解析Skill正文，不执行工具，只在Assistant消息分支按本次ToolSpec和tool_dynamic授权模型工具调用。
ToolPlugin的PendingToolCalls只负责异步请求与响应关联，不是MCL工具批次完成的事实来源；完成状态由MCL pending_tool数组决定。
InferencePlugin执行InferenceRequestEvent，Provider Adapter保留tool_name并发布AgentMessage::Assistant。
MemoryPlugin只消费历史和实时上下文事件，不读取AgentStatus或ToolPlugin的Pending状态。
```

## 持有关系

```text
World
├── AgentCreateRequest -> AgentCreated + AgentCreateResult
├── InjectAgentVisibleResource -> AgentVisibleResourceInjected | AgentVisibleResourceInjectionFailed
├── RemoveAgentVisibleResource -> AgentVisibleResourceRemoved
├── RestoreAgentDefaultVisibility -> AgentVisibleResourceInjected * N | AgentVisibleResourceInjectionFailed * N
├── RemoveAllAgentVisibleResources -> AgentVisibleResourceRemoved * N
├── PendingVisibilityCommands Resource
├── PendingInferenceToolSchemas Resource
├── PendingContextCompactions Resource
├── AgentContextCompactRequest -> context_compaction_system
├── ContextCompactionInferenceResponse -> context_compaction_system
├── AgentMessage -> agent_message_system -> MclRuntimeMessage -> Base Driver mailbox
└── Agent Entity
    ├── AgentIdentity
    ├── AgentWorkspaceId
    ├── AgentMcl--MclPlugin所有
    ├── AgentStatus
    │   ├── turn_id
    └── AgentResourceMap--ToolPlugin所有
```
