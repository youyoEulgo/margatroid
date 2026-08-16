# AgentPlugin

## 类型

公开：
```text
AgentPlugin：Agent实例与消息循环插件，公开结构体--安装创建、消息处理和工具批次完成System
    schedule: String--System所属Schedule，私有
    new() -> Self
    with_schedule(mut self, schedule: impl Into<String>) -> Self
    impl Default for AgentPlugin
    impl Plugin for AgentPlugin
        build(self, app: &mut App)
            构建插件：要求RuntimePlugin、ToolPlugin和目标Schedule存在，插入InFlightVisibilityRegistrations，挂载Agent创建、可见性修改、资源注册响应、Skill状态、消息处理和工具批次完成System

AgentCreateRequest：Agent创建请求，公开事件--WorkspacePlugin交付Agent自有字段
    id: String--Workspace创建子请求ID
    agent_id: ResourceId--稳定Agent资源ID，格式agent:<workspace>/<name>:latest
    workspace_id: Entity--所属Workspace Entity
    system_prompt: String--当前系统提示词
    messages: Vec<Message>--恢复的长期User与Assistant上下文
    tool_context: Vec<Message>--恢复的当前轮Tool上下文
    default_visibility: BTreeSet<ResourceId>--创建时默认可见资源
    impl Event for AgentCreateRequest

AgentCreateResult：Agent Entity创建结果，公开事件--无论成功失败都恰好发送一次
    id: String--原创建子请求ID
    agent_id: ResourceId--原请求中的稳定Agent资源ID
    result: Result<Entity, AgentCreateError>--成功时为已经挂载Agent自有组件和空AgentToolMap的Entity
    impl Event for AgentCreateResult

AgentCreated：Agent Entity创建成功通知，公开事件--Agent自有组件和空AgentToolMap已经建立，且已经发出默认可见性恢复请求
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

RestoreAgentDefaultVisibility：恢复默认可见性，公开事件--AgentPlugin在Agent Entity创建成功时自行发送；其他调用方也可显式请求，不等待资源注入
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
    ToolMapSetupFailed

AgentCreateError：Agent创建错误，公开结构体--不包含上下文正文
    kind: AgentCreateErrorKind
    message: String
    kind(&self) -> AgentCreateErrorKind
    message(&self) -> &str

AgentVisibilityErrorKind：可见性错误分类，公开枚举
    AgentMissing
    VisibilityMissing
    ToolMapMissing
    RegistrationFailed
    RegistrationResponseMismatch

AgentVisibilityError：可见性错误，公开结构体--稳定描述Agent、注册和回执错误，不包含资源正文
    kind: AgentVisibilityErrorKind
    message: String
    kind(&self) -> AgentVisibilityErrorKind
    message(&self) -> &str

LoadAgentSkill：加载持久Skill事件，公开事件--把Skill加入Agent每轮自动调用集合
    id: String--请求ID，用于失败定位
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--完整Skill资源ID
    impl Event for LoadAgentSkill

UnloadAgentSkill：卸载持久Skill事件，公开事件--把Skill移出Agent每轮自动调用集合
    id: String--请求ID，用于失败定位
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--完整Skill资源ID
    impl Event for UnloadAgentSkill

UnloadAllAgentSkills：卸载全部持久Skill事件，公开事件--清空Agent每轮自动调用集合
    id: String--请求ID，用于失败定位
    agent: Entity--目标Agent Entity
    impl Event for UnloadAllAgentSkills

AgentIdentity：Agent稳定身份，公开Component
    id: ResourceId--type=agent的唯一资源ID，私有
    id(&self) -> &ResourceId
    impl Component for AgentIdentity

AgentWorkspaceId：Agent所属Workspace，公开Component
    workspace_id: Entity--Workspace Entity，私有
    workspace_id(&self) -> Entity
    impl Component for AgentWorkspaceId

AgentContext：Agent上下文，公开Component--保存系统提示词、长期对话和当前轮工具上下文
    system_prompt: String--当前系统提示词，私有
    messages: Vec<Message>--长期User与Assistant消息，私有
    tool_context: Vec<Message>--当前轮Tool响应，私有
    system_prompt(&self) -> &str
    messages(&self) -> &[Message]
    tool_context(&self) -> &[Message]
    append_message(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender)
        追加对话：公开方法，只接受User或Assistant，修改后发送AgentContextMessagesUpdated
    rewrite_messages(&mut self, agent: Entity, messages: Vec<Message>, events: &RuntimeEventSender)
        重写对话：公开方法，整体替换长期上下文并发送AgentContextMessagesUpdated
    append_tool_context(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender)
        追加工具上下文：公开方法，只接受Tool并发送AgentContextMessagesUpdated
    clear_tool_context(&mut self, agent: Entity, events: &RuntimeEventSender)
        清空工具上下文：公开方法，修改后发送AgentContextMessagesUpdated
    impl Component for AgentContext

AgentDefaultVisibility：Agent默认可见性，公开只读Component
    resources: BTreeSet<ResourceId>--创建时确定的资源集合，私有
    resources(&self) -> &BTreeSet<ResourceId>
    impl Component for AgentDefaultVisibility

AgentDynamicVisibility：Agent动态可见性，公开Component--当前实际可见资源
    resources: BTreeSet<ResourceId>--当前资源集合，私有
    resources(&self) -> &BTreeSet<ResourceId>
    impl Component for AgentDynamicVisibility
    限制：只有AgentPlugin的可见性System可以修改resources；公开接口只读

AgentPluginInstalled：安装标记，公开Resource

WorldAgentExt：World Agent扩展，公开trait
    agent(&self, id: &ResourceId) -> Option<Entity>
        按身份查询：公开方法，返回稳定资源ID匹配且仍存活的Agent Entity
    agent_loading_skills(&self, agent: Entity) -> Option<&BTreeSet<ResourceId>>
        查询持久Skill：公开只读方法，不暴露AgentStatus其他字段
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
AgentStatus：Agent轮次与持久Skill状态，crate公开Component--不保存pending tool
    turn_id: Option<String>--当前仍在处理的交互轮次；空表示Agent空闲
    loading_skills: BTreeSet<ResourceId>--每轮自动调用的Skill资源ID
    begin_turn(&mut self, turn_id: String) -> Result<(), AgentStepError>
        开始轮次：拒绝与当前未完成轮次重叠
    finish_turn(&mut self, turn_id: &str) -> Result<(), AgentStepError>
        完成轮次：只允许完成当前turn
    load_skill(&mut self, resource_id: ResourceId) -> Result<(), AgentStepError>
        加载Skill：只接受type=skill并按完整ResourceId去重
    unload_skill(&mut self, resource_id: &ResourceId) -> bool
    unload_all_skills(&mut self)
    impl Component for AgentStatus
```

私有：
```text
AvailableTools：一次推理的临时工具规格集合，私有结构体
    definitions: Vec<ToolDefinition>--从当前AgentToolMap取得的内部ToolSpec

PendingInferenceToolSchemas：飞行中推理的ToolSpec快照，私有Resource
    schemas: HashMap<(Entity, String), Vec<ToolDefinition>>--按Agent与turn_id关联下一条Assistant

ConversationTurnResult：单条消息处理结果，私有枚举
    WaitForTools--已发送ToolCallEvent，等待ToolPlugin完成批次
    FinishTurn--Assistant无工具调用，本轮结束
    RequestInference--上下文完整，可发送InferenceRequestEvent

AgentStepError：Agent处理错误，私有枚举
    AgentMissing
    IdentityMissing
    ContextMissing
    StatusMissing
    ToolMapMissing
    InvalidMessage
    InvalidTurn
    InvalidToolCall
    Tool(ToolError)
    failure_message(&self) -> String
        构造稳定有界错误描述，不包含消息正文、工具参数或资源正文

InFlightVisibilityRegistrations：飞行中的资源注册，私有Resource--只保存异步请求关联，不是PendingToolCalls，也不保存可见性操作快照
    registrations: Map<(Entity, ResourceId), InFlightVisibilityRegistration>--同一Agent同一资源最多一个底层注册请求

InFlightVisibilityRegistration：单项飞行中注册，私有结构体
    registration_id: String--发送给ToolPlugin注册协议的内部唯一请求ID
    agent: Entity
    resource_id: ResourceId
    notification_ids: BTreeSet<String>--等待注入成功或失败通知的外部操作ID
    desired_visible: bool--响应返回时是否仍应注入；删除操作可改为false，后续再次注入可恢复为true
```

## 函数

```text
agent_create_system(world: &mut World)
    创建Agent：私有System，读取AgentCreateRequest
    行为：
        验证请求ID、agent_id、Workspace Entity及恢复消息结构；失败时发送AgentCreateResult::Err
        创建Entity并挂载AgentIdentity、AgentWorkspaceId、AgentContext、AgentDefaultVisibility、空AgentDynamicVisibility和空AgentStatus
        调用ToolPlugin公开函数挂载空AgentToolMap；失败时despawn已创建Entity并发送AgentCreateResult::Err
        不直接注册资源；此时WorkspacePlugin尚未挂载AgentToolEnvironment
        成功时发送AgentCreated，并调用自身restore_agent_default_visibility；注册请求在后续System帧处理
        最后发送AgentCreateResult::Ok(agent)

agent_visibility_change_system(world: &mut World)
    修改动态可见性：私有System，分别读取InjectAgentVisibleResource、RemoveAgentVisibleResource、SetAgentDefaultResourceVisibility、RestoreAgentDefaultVisibility和RemoveAllAgentVisibleResources
    行为：
        Inject逐条调用inject_visible_resource
        Remove逐条调用remove_visible_resource，目标不存在时仍发送幂等成功删除通知
        SetDefault先验证resource_id属于AgentDefaultVisibility；否则发送AgentFailure且不修改；成功后按visible调用注入或删除
        Restore先调用remove_all_visible_resources，再按AgentDefaultVisibility顺序逐项调用inject_visible_resource
        RemoveAll调用remove_all_visible_resources
        任一资源失败只发送对应错误通知；不结束Agent、不结束Workspace、不回滚其他资源

inject_visible_resource(world: &mut World, id: &str, agent: Entity, resource_id: &ResourceId, events: &RuntimeEventSender)
    注入单项资源：私有函数
    行为：
        验证AgentDynamicVisibility和AgentToolMap；失败时发送AgentVisibleResourceInjectionFailed
        资源已经动态可见时直接发送AgentVisibleResourceInjected，操作幂等成功
        AgentToolMap中该resource_id恰好存在一个映射时插入AgentDynamicVisibility并发送AgentVisibleResourceInjected
        AgentToolMap中存在多个同资源映射时发送AgentVisibleResourceInjectionFailed
        映射不存在且已有同Agent同资源飞行中注册时追加id到notification_ids并把desired_visible设为true
        映射不存在且没有飞行中注册时生成内部registration_id，写入InFlightVisibilityRegistrations并发送AgentToolRegisterRequest

remove_visible_resource(world: &mut World, id: &str, agent: Entity, resource_id: &ResourceId, events: &RuntimeEventSender)
    删除单项资源：私有函数
    行为：
        从AgentDynamicVisibility删除resource_id；目标不存在时仍视为幂等成功
        同Agent同资源存在飞行中注册时把desired_visible设为false
        resource_id.type=skill时同时从AgentStatus.loading_skills删除
        不删除AgentToolMap，不回收tool_name或next_index
        发送AgentVisibleResourceRemoved { id, agent, resource_id }

remove_all_visible_resources(world: &mut World, id: &str, agent: Entity, events: &RuntimeEventSender)
    删除全部资源：私有函数
    行为：
        取得并清空AgentDynamicVisibility，按ResourceId顺序为原来实际可见的每个资源发送AgentVisibleResourceRemoved
        把该Agent全部飞行中注册的desired_visible设为false
        动态可见性原本为空时不发送资源删除通知
        不删除AgentToolMap
        显式RemoveAll时清空AgentStatus.loading_skills；Restore内部清空不修改loading_skills

collect_agent_tool_registration_system(world: &mut World)
    收集资源注册响应：私有System，读取AgentToolRegisterResponse
    行为：
        按响应id查InFlightVisibilityRegistrations，并校验agent和resource_id完全一致
        无匹配时记录稳定错误，不修改动态可见性
        已匹配但agent或resource_id字段不一致时，为该注册的notification_ids发送AgentVisibleResourceInjectionFailed并删除飞行中记录
        注册失败时为notification_ids中的每个ID发送AgentVisibleResourceInjectionFailed并删除飞行中记录
        注册成功时再次确认Agent仍存活且AgentToolMap中该resource_id恰好存在一个映射；校验失败按响应不匹配处理
        desired_visible=true时插入AgentDynamicVisibility，并为notification_ids中的每个ID发送AgentVisibleResourceInjected
        desired_visible=false时只保留AgentToolMap，不注入资源，也不发送注入成功通知
        完成后删除飞行中记录

cleanup_dead_agent_registrations_system(world: &mut World)
    清理死亡Agent注册：私有无输入System
    行为：删除agent已经死亡的全部InFlightVisibilityRegistration与PendingInferenceToolSchemas快照；迟到响应将作为无匹配响应被丢弃

agent_skill_state_system(world: &mut World)
    修改持久Skill：私有System，读取LoadAgentSkill、UnloadAgentSkill和UnloadAllAgentSkills
    行为：
        Load要求Agent存活且resource_id.type=skill，调用AgentStatus.load_skill并按完整ResourceId去重
        Unload要求Agent存活且resource_id.type=skill，调用AgentStatus.unload_skill；目标不存在时仍视为成功
        UnloadAll要求Agent存活，调用AgentStatus.unload_all_skills
        不执行Skill，不改变可见性或AgentToolMap
        Agent不存在或资源类型非法时发送AgentFailure { kind: Agent }

agent_message_system(world: &mut World)
    处理Agent消息：私有System，读取AgentMessage并逐条调用handle_agent_message
    行为：失败时发送AgentFailure { kind: Agent }，不伪造Assistant或Tool消息

handle_agent_message(world: &mut World, event: &AgentMessage, events: &RuntimeEventSender) -> Result<ConversationTurnResult, AgentStepError>
    处理消息：私有函数
    行为：
        System返回InvalidMessage
        User开始或确认当前turn，清空上一轮tool_context，写历史并追加长期messages
        User.tool_calls与当前loading_skills实例合并；有调用时发送ToolCallEvent，无调用时发送InferenceRequestEvent
        Assistant先取得该turn对应的PendingInferenceToolSchemas；写历史并追加长期messages
        Assistant.tool_calls为空时结束当前turn
        Assistant.tool_calls非空时逐项检查tool_name存在于本次ToolSpec且映射资源仍在AgentDynamicVisibility；不满足时直接发送Message::Tool拒绝响应，提示模型检查当前ToolSpec，不发送ToolCallEvent
        Assistant所有调用均被拒绝时发送ToolTurnCompleted，等待拒绝响应进入tool_context后重新推理
        Assistant鉴权通过的Skill资源立即加入loading_skills，再与普通tool_calls合并后统一派发
        Tool写历史并追加tool_context；不修改loading_skills；不判断pending数量，不直接发起下一次推理

tool_turn_completed_system(world: &mut World)
    处理工具批次完成：私有System，读取ToolTurnCompleted
    行为：验证事件turn_id等于AgentStatus当前turn；随后使用现有上下文发送InferenceRequestEvent

take_pending_tool_schema(world: &mut World, agent: Entity, turn_id: &str) -> Vec<ToolDefinition>
    取得推理工具规格：私有函数，从PendingInferenceToolSchemas移除并返回当前Agent与turn对应的ToolSpec；不存在时返回空数组

record_history_message(world: &mut World, event: &AgentMessage, events: &RuntimeEventSender, tool_schema: Vec<ToolDefinition>)
    请求历史写入：私有函数
    行为：User原样发送且tool_schema为空；Assistant原样发送并携带传入的ToolSpec；Skill类型Tool保留resource_id和tool_call_id并把content替换为resource_id.to_string()；非Skill类型Tool原样发送；Tool的tool_schema为空
    限制：Skill正文只进入实时tool_context，不进入历史事件；非Skill工具响应正文完整写入历史事件

append_conversation_message(world: &mut World, agent: Entity, message: Message, events: &RuntimeEventSender) -> Result<(), AgentStepError>
append_tool_context(world: &mut World, agent: Entity, message: Message, events: &RuntimeEventSender) -> Result<(), AgentStepError>
clear_tool_context(world: &mut World, agent: Entity, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    上下文修改：私有函数，分别调用AgentContext的唯一修改入口

build_available_tools(world: &World, agent: Entity) -> Result<AvailableTools, AgentStepError>
    构造工具规格：私有函数
    行为：
        读取AgentDynamicVisibility和同一Entity上的AgentToolMap
        按动态可见性ResourceId顺序调用AgentToolMap.get_by_resource
        每个可见资源必须恰好对应一个已注册ToolMap，否则失败
        克隆ToolMap.template形成内部ToolSpec列表；不转换Provider格式

expand_loading_skills(world: &World, agent: Entity) -> Result<Vec<ToolCall>, AgentStepError>
    展开持久Skill：私有函数
    行为：
        遍历AgentStatus.loading_skills
        按resource_id从AgentToolMap取得唯一ToolMap
        为每个Skill生成新的Margatroid工具调用ID
        构造ToolCall { id, tool_name: map.tool_name, arguments: "{}" }

dispatch_tool_calls(world: &World, turn_id: &str, agent: Entity, explicit: &[ToolCall], include_loading_skills: bool, events: &RuntimeEventSender) -> Result<ConversationTurnResult, AgentStepError>
    派发工具：私有函数
    行为：
        验证显式调用ID和tool_name非空，且tool_name存在于当前AgentToolMap
        按需合并本轮新实例化的loading skills
        验证同批ToolCall.id唯一
        为每个调用发送ToolCallEvent { turn_id, agent, call }
        没有调用时返回RequestInference，否则返回WaitForTools

dispatch_assistant_tool_calls(world: &mut World, turn_id: &str, agent: Entity, explicit: &[ToolCall], tool_schema: &[ToolDefinition], events: &RuntimeEventSender) -> Result<ConversationTurnResult, AgentStepError>
    校验并派发模型调用：私有函数
    行为：ToolMap无法解析或调用ID重复时返回InvalidToolBatch；ToolSpec中没有tool_name或映射资源不在动态可见性时发送Message::Tool { content: TOOL_PERMISSION_DENIED }；Skill资源通过鉴权后加入loading_skills；其余通过校验的调用交给dispatch_tool_calls

build_inference_context(world: &World, agent: Entity) -> Result<Vec<Message>, AgentStepError>
    组装上下文：固定按System、messages、tool_context返回

send_inference_request(world: &mut World, turn_id: &str, agent: Entity, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    发起推理：构造当前内部ToolSpec与完整上下文；把ToolSpec按(agent, turn_id)写入PendingInferenceToolSchemas后发送InferenceRequestEvent
    限制：只读取调用时刻的AgentDynamicVisibility；正在注册或注册失败的资源不进入本次请求
```

## 逻辑

```text
创建与可见性：
    AgentCreateRequest
        -> 创建Agent自有组件、空动态可见性和空AgentToolMap
        -> AgentCreated + AgentCreateResult
        -> RestoreAgentDefaultVisibility
        -> 清空动态可见性
        -> 默认资源逐项注入
           ├── ToolMap已存在 -> AgentVisibleResourceInjected
           └── ToolMap不存在 -> AgentToolRegisterRequest
                                  -> AgentToolRegisterResponse
                                  ├── 成功且仍期望可见 -> AgentVisibleResourceInjected
                                  ├── 成功但期间已删除 -> 只保留ToolMap
                                  └── 失败 -> AgentVisibleResourceInjectionFailed

User：
    AgentMessage::User
        -> begin_turn
        -> 清空tool_context、记录历史、追加messages
        -> 合并User.tool_calls与本轮loading skill调用
        -> 有工具：ToolCallEvent并等待
        -> 无工具：InferenceRequestEvent

Assistant：
    AgentMessage::Assistant
        -> 记录历史、追加messages
        -> 无tool_calls：finish_turn
        -> 有tool_calls：合并本轮loading skill调用并发送ToolCallEvent

Tool：
    AgentMessage::Tool { resource_id, tool_call_id, content }
        -> 记录历史并追加tool_context
        -> 不修改loading_skills
        -> 不查询pending，不判断批次完成

批次完成：
    ToolPlugin移除最后一个Pending请求
        -> ToolTurnCompleted
        -> AgentPlugin发送InferenceRequestEvent

loading skill：
    AgentStatus只保存Skill ResourceId
        -> 每轮从AgentToolMap恢复tool_name
        -> 每轮生成新的ToolCall.id
        -> 进入与模型主动调用相同的ToolCallEvent链路
```

## 边界

```text
AgentPlugin负责Agent创建、上下文、动态可见性、当前turn、loading skills、内部ToolSpec构造和推理调度。
AgentPlugin逐资源修改动态可见性，不保存完整可见性操作快照，不因单项资源注册失败结束Agent或Workspace。
WorkspacePlugin只在收到AgentCreateResult后挂载外部运行组件，不介入资源注册或可见性修改。
AgentPlugin不保存pending tool，不解析Skill正文，不执行工具；只在Assistant消息分支按本次ToolSpec和动态可见性授权模型工具调用。
ToolPlugin拥有AgentToolMap和PendingToolCalls，并通过ToolTurnCompleted通知批次结束。
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
├── InFlightVisibilityRegistrations Resource
├── PendingInferenceToolSchemas Resource
├── AgentMessage -> agent_message_system
├── ToolTurnCompleted -> tool_turn_completed_system
└── Agent Entity
    ├── AgentIdentity
    ├── AgentWorkspaceId
    ├── AgentContext
    ├── AgentDefaultVisibility
    ├── AgentDynamicVisibility
    ├── AgentStatus
    │   ├── turn_id
    │   └── loading_skills: BTreeSet<ResourceId>
    └── AgentToolMap--ToolPlugin所有
```
