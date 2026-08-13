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
            构建插件：要求RuntimePlugin和目标Schedule存在，挂载agent_create_system、agent_skill_state_system、agent_message_system和tool_turn_completed_system

AgentCreateRequest：Agent创建请求，公开事件--WorkspacePlugin交付Agent自有字段
    id: String--Workspace创建子请求ID
    agent_id: ResourceId--稳定Agent资源ID，格式agent:<workspace>/<name>:latest
    workspace_id: Entity--所属Workspace Entity
    system_prompt: String--当前系统提示词
    messages: Vec<Message>--恢复的长期User与Assistant上下文
    tool_context: Vec<Message>--恢复的当前轮Tool上下文
    default_visibility: BTreeSet<ResourceId>--创建时默认可见资源
    impl Event for AgentCreateRequest

AgentCreated：Agent创建完成回执，公开事件
    id: String--原创建子请求ID
    agent: Entity--新Agent Entity
    impl Event for AgentCreated

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

AgentPluginInstalled：安装标记，公开Resource

WorldAgentExt：World Agent扩展，公开trait
    agent(&self, id: &ResourceId) -> Option<Entity>
        按身份查询：公开方法，返回稳定资源ID匹配且仍存活的Agent Entity
    agent_loading_skills(&self, agent: Entity) -> Option<&BTreeSet<ResourceId>>
        查询持久Skill：公开只读方法，不暴露AgentStatus其他字段
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
```

## 函数

```text
agent_create_system(world: &mut World)
    创建Agent：私有System，读取AgentCreateRequest
    行为：
        验证请求ID、agent_id、Workspace Entity及恢复消息结构
        创建Entity并挂载AgentIdentity、AgentWorkspaceId、AgentContext、两层Visibility和空AgentStatus
        AgentDynamicVisibility初始完整复制AgentDefaultVisibility
        不挂载AgentToolMap；该Component由ToolPlugin所有并由WorkspacePlugin在创建回执后挂载
        发送AgentCreated { id, agent }

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
        Assistant写历史并追加长期messages；tool_calls为空时结束当前turn
        Assistant.tool_calls非空时与loading_skills实例合并并发送ToolCallEvent
        Tool写历史并追加tool_context；resource_id.type=skill时将resource_id加入loading_skills；不判断pending数量，不直接发起下一次推理

tool_turn_completed_system(world: &mut World)
    处理工具批次完成：私有System，读取ToolTurnCompleted
    行为：验证事件turn_id等于AgentStatus当前turn；随后使用现有上下文发送InferenceRequestEvent

record_history_message(event: &AgentMessage, events: &RuntimeEventSender)
    请求历史写入：私有函数
    行为：User和Assistant原样发送；Tool保留resource_id和tool_call_id，并把content直接替换为resource_id.to_string()
    限制：工具正文只进入实时tool_context，不进入历史事件

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

build_inference_context(world: &World, agent: Entity) -> Result<Vec<Message>, AgentStepError>
    组装上下文：固定按System、messages、tool_context返回

send_inference_request(world: &World, turn_id: &str, agent: Entity, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    发起推理：构造当前内部ToolSpec与完整上下文，发送InferenceRequestEvent
```

## 逻辑

```text
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
        -> resource_id.type=skill时加入loading_skills
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
AgentPlugin不保存pending tool，不把tool_name转换为ResourceId，不解析Skill正文，不执行工具。
ToolPlugin拥有AgentToolMap和PendingToolCalls，并通过ToolTurnCompleted通知批次结束。
InferencePlugin执行InferenceRequestEvent，Provider Adapter保留tool_name并发布AgentMessage::Assistant。
MemoryPlugin只消费历史和实时上下文事件，不读取AgentStatus或ToolPlugin的Pending状态。
```

## 持有关系

```text
World
├── AgentCreateRequest -> AgentCreated
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
