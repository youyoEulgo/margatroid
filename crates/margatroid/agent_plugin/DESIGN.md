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

# AgentPlugin

## 统一资源身份约定

```text
Agent的稳定身份使用ResourceId：agent:<workspace>/<name>:latest；动态Subagent不属于本阶段
AgentIdentity保存ResourceId，不保存workspace.name0等旧格式字符串
AgentDefaultVisibility、AgentDynamicVisibility和PendingToolCall中的资源字段使用ResourceId
core_plugin::Entity仅用于当前World内的组件和事件关联
```

## 类型

公开：
```text
AgentPlugin：Agent实例与工具消息循环插件，公开结构体--安装创建和AgentToolCall处理System
    schedule: String--两个System所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，默认使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，替换默认Schedule并返回自身
    impl Default for AgentPlugin
        Default：公开trait实现，与new等价
    impl Plugin for AgentPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：要求RuntimePlugin和目标Schedule存在，依次挂载agent_create_system和agent_tool_call_system

AgentCreateRequest：Agent创建请求，公开事件--WorkspacePlugin交付创建实例所需的Agent自有字段
    id: String--Workspace生成的创建子请求ID
    agent_id: ResourceId--type=agent的稳定Agent资源ID，例如agent:demo/coder:latest
    workspace_id: Entity--Agent所属Workspace Entity
    system_prompt: String--当前系统提示词
    messages: Vec<Message>--恢复的长期对话上下文
    tool_context: Vec<Message>--恢复的当前轮工具上下文
    default_visibility: BTreeSet<ResourceId>--创建时默认可见资源
    impl Event for AgentCreateRequest
        Event：公开trait实现

AgentCreated：Agent创建完成回执，公开事件--把创建子请求与新Entity关联
    id: String--原创建子请求ID
    agent: Entity--新Agent Entity
    impl Event for AgentCreated
        Event：公开trait实现

AgentIdentity：Agent稳定身份，公开组件--保存跨Plugin和跨进程使用的逻辑Agent ID
    id: ResourceId--type=agent的唯一资源ID，私有
    id(&self) -> &ResourceId
        取得Agent ID：公开方法
    impl Component for AgentIdentity
        Component：公开trait实现

AgentWorkspaceId：Agent所属Workspace标识，公开组件
    workspace_id: Entity--Workspace Entity，私有
    workspace_id(&self) -> Entity
        取得Workspace：公开方法
    impl Component for AgentWorkspaceId
        Component：公开trait实现

AgentContext：Agent上下文，公开组件--保存系统提示词、长期对话和当前轮临时工具上下文
    system_prompt: String--当前系统提示词，私有
    messages: Vec<Message>--长期User和Assistant对话，私有
    tool_context: Vec<Message>--当前轮Tool响应与Skill正文，私有
    system_prompt(&self) -> &str
        取得系统提示词：公开只读方法
    messages(&self) -> &[Message]
        取得长期对话：公开只读方法
    tool_context(&self) -> &[Message]
        取得工具上下文：公开只读方法
    append_message(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender)
        追加对话：公开方法，只接受User或Assistant，修改后发送AgentContextMessagesUpdated
    rewrite_messages(&mut self, agent: Entity, messages: Vec<Message>, events: &RuntimeEventSender)
        重写对话：公开方法，整体替换长期上下文并发送AgentContextMessagesUpdated
    append_tool_context(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender)
        追加工具上下文：公开方法，只接受Tool并发送AgentContextMessagesUpdated
    clear_tool_context(&mut self, agent: Entity, events: &RuntimeEventSender)
        清空工具上下文：公开方法，清空后发送AgentContextMessagesUpdated
    impl Component for AgentContext
        Component：公开trait实现

AgentDefaultVisibility：Agent默认可见性，公开组件--创建时确定后只读
    resources: BTreeSet<ResourceId>--默认可见资源，私有
    resources(&self) -> &BTreeSet<ResourceId>
        取得默认可见性：公开只读方法
    impl Component for AgentDefaultVisibility
        Component：公开trait实现

AgentDynamicVisibility：Agent动态可见性，公开组件--当前实际可见资源
    resources: BTreeSet<ResourceId>--动态可见资源，私有
    resources(&self) -> &BTreeSet<ResourceId>
        取得动态可见性：公开只读方法
    impl Component for AgentDynamicVisibility
        Component：公开trait实现

AgentPluginInstalled：AgentPlugin安装标记，公开单元Resource--阻止重复安装并供WorkspacePlugin确认依赖

WorldAgentExt：World Agent扩展，公开trait--按稳定资源ID查询当前World中的Agent Entity
    agent(&self, id: &ResourceId) -> Option<Entity>
        查询Agent：公开方法，只返回身份完整匹配且仍存在的Agent Entity
    impl WorldAgentExt for World
        WorldAgentExt for World：公开trait实现，遍历AgentIdentity并按完整ResourceId匹配
```

crate公开：
```text
AgentStatus：Agent工具调用状态，crate公开组件--同时保存当前工具批次和持久化Skill调用模板
    pending_tools: BTreeMap<String, PendingToolCall>--当前轮尚未完成的工具调用
    loading_skills: BTreeMap<String, PendingToolCall>--每轮自动展开的Skill调用模板
    add_tool_call(&mut self, call: PendingToolCall) -> Result<(), AgentStepError>
        添加工具调用：crate公开方法，向pending_tools加入唯一调用
    complete_tool_call(&mut self, id: &str) -> ToolCallCompletion
        完成工具调用：crate公开方法，移除调用并返回Invalid、Pending或Completed
    load_skill(&mut self, call: PendingToolCall) -> Result<(), AgentStepError>
        加载Skill：crate公开方法，把Skill工具调用模板加入loading_skills
    unload_skill(&mut self, key: &str) -> bool
        卸载Skill：crate公开方法，删除一个模板
    unload_all_skills(&mut self)
        卸载全部Skill：crate公开方法
    impl Component for AgentStatus
        Component：crate公开trait实现

PendingToolCall：工具调用状态，crate公开结构体--pending_tools和loading_skills共用的内部值类型
    call: ToolCall--工具调用信息，持久模板展开时重新生成ID
    resource: ResourceId--工具资源ID
    kind: ToolCallKind--普通工具或Skill

ToolCallKind：工具调用来源，crate公开枚举
    Tool--普通工具调用
    Skill--Skill工具调用

ToolCallCompletion：工具完成结果，crate公开枚举
    Invalid--调用ID不属于当前pending批次
    Pending--本次完成后仍有其他工具未完成
    Completed--当前批次全部完成
```

私有：
```text
AvailableTools：一次请求的临时工具集合，私有结构体--不是Agent组件
    definitions: Vec<ToolDefinition>--发给InferencePlugin的工具定义，name为完整ResourceId
    resources: BTreeSet<ResourceId>--当次定义对应的完整资源集合

ConversationTurnResult：当前AgentMessage处理结果，私有枚举--只在一次System执行中使用
    WaitForTools--仍有pending_tools
    FinishTurn--Assistant没有工具调用，本轮结束
    RequestInference--上下文完整，可发送InferenceRequest

AgentStepError：当前事件处理错误，私有枚举--转换为AgentFailure
    AgentMissing
    ContextMissing
    StatusMissing
    InvalidMessage
    InvalidToolBatch
    Tool(ToolError)
    failure_message(&self) -> String
        构造失败描述：私有方法，返回不包含消息正文、工具参数或资源正文的稳定有界文本
```

## 函数

私有：
```text
agent_create_system(world: &mut World)
    创建Agent：私有System，读取AgentCreateRequest并创建Agent Entity
    行为：
        要求id、agent_id非空，workspace_id存活
        messages只允许User和Assistant，tool_context只允许Tool
        挂载AgentIdentity、AgentWorkspaceId、AgentContext、两层Visibility和AgentStatus
        AgentDynamicVisibility初始完整复制AgentDefaultVisibility
        发送AgentCreated { id, agent }
        不读取Workspace、AgentImage或磁盘，不挂载其他Plugin拥有的组件

agent_tool_call_system(world: &mut World)
    处理Agent消息：私有System，统一处理消息和工具分支
    行为：
        收集本次全部AgentMessage，结束EventReader借用
        对每个消息调用handle_agent_message
        AgentMessage.agent必须是存活Entity
        失败时发送AgentFailure { kind: Agent }，不伪造Assistant或Tool消息
        只根据Message变体和tool_calls列表决定后续动作

handle_agent_message(world: &mut World, event: &AgentMessage, events: &RuntimeEventSender) -> Result<ConversationTurnResult, AgentStepError>
    处理单条消息：私有函数，根据Message变体完成记忆、工具和推理调度
    行为：
        System直接返回InvalidMessage
        User先清空tool_context，发送历史事件并追加messages，合并User.tool_calls与loading_skills
        Assistant先清空tool_context，发送历史事件并追加messages；无tool_calls时结束，否则合并Assistant.tool_calls与loading_skills
        Tool根据tool_call_id查询pending条目；普通工具原样记录历史，Skill以加载标记记录历史；完整响应追加tool_context
        Tool分支不清空tool_context；仍有pending时等待，全部完成后发起推理
        对话结束只是当前System内的局部结果，不写入AgentStatus

record_history_message(event: &AgentMessage, pending: Option<&PendingToolCall>, events: &RuntimeEventSender)
    请求历史写入：私有函数，包装AgentHistoryMessageWriteRequested
    行为：
        User和Assistant原样发送
        普通Tool原样发送
        Skill Tool使用原tool_call_id，content替换为"skill: <scope/name> loaded"
        Skill正文永不进入历史事件

append_conversation_message(world: &mut World, agent: Entity, message: Message, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    追加长期对话：私有函数，验证Agent和AgentContext后调用AgentContext.append_message

append_tool_context(world: &mut World, agent: Entity, message: Message, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    追加工具上下文：私有函数，调用AgentContext.append_tool_context

clear_tool_context(world: &mut World, agent: Entity, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    清空工具上下文：私有函数，调用AgentContext.clear_tool_context

build_available_tools(world: &World, agent: Entity) -> Result<AvailableTools, AgentStepError>
    构造可用工具：私有函数，每次从AgentDynamicVisibility构造工具定义和名称映射
    行为：按资源顺序调用WorldToolExt::tool_definition_for；每个定义的name保持完整ResourceId，不合并同类型资源，不把结果保存为Component

dispatch_tool_calls(world: &mut World, id: &str, agent: Entity, tool_calls: &[ToolCall], include_loading_skills: bool, events: &RuntimeEventSender) -> Result<ConversationTurnResult, AgentStepError>
    组建工具批次：私有函数，拒绝重叠批次，解析显式调用，按参数合并loading_skills，验证ID唯一后写入pending_tools并派发
    返回：没有任何工具时返回RequestInference，否则返回WaitForTools

expand_loading_skills(status: &AgentStatus) -> Vec<PendingToolCall>
    展开持久Skill：私有函数，复制loading_skills模板并为本轮重新生成调用ID

queue_tool_calls(available_tools: &AvailableTools, calls: &[ToolCall]) -> Result<Vec<PendingToolCall>, AgentStepError>
    解析工具调用：私有函数，确认ToolCall.resource属于当次AvailableTools并根据resource_type构造Tool或Skill类型的PendingToolCall

dispatch_pending_tools(world: &World, id: &str, agent: Entity, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    派发待完成工具：私有函数，逐个发送携带轮次ID、Agent Entity、ResourceId和ToolCall的ToolCallRequest

tool_call_kind(resource: &ResourceId) -> ToolCallKind
    判断工具来源：私有函数，resource_type为skill时返回Skill，其他类型返回Tool

skill_key(resource: &ResourceId) -> String
    构造Skill模板键：私有函数，使用完整ResourceId生成loading_skills稳定键

build_inference_context(world: &World, agent: Entity) -> Result<Vec<Message>, AgentStepError>
    组装推理上下文：私有函数，固定按System、messages、tool_context返回
    行为：Skill正文只来自当前tool_context，不在组装时重复读取

send_inference_command(world: &World, id: &str, agent: Entity, events: &RuntimeEventSender) -> Result<(), AgentStepError>
    发起推理：私有函数，构造ResourceId形式的可见工具定义和完整上下文后发送InferenceRequest
    行为：只克隆当前上下文，不修改AgentContext；只有后续User或Assistant进入System时才清空tool_context

assert_conversation_message(message: &Message)
    验证长期消息：私有函数，非User或Assistant时panic

assert_conversation_messages(messages: &[Message])
    验证长期快照：私有函数，逐条调用assert_conversation_message
```

## 逻辑

```text
用户消息：
    AgentMessage { User { content, tool_calls } }
        -> 清空上一轮tool_context
        -> 发送AgentHistoryMessageWriteRequested并追加messages
        -> 展开loading_skills并合并User.tool_calls
        -> pending_tools为空时发送InferenceRequest
        -> pending_tools非空时发送ToolCallRequest并等待

Assistant消息：
    AgentMessage { Assistant { content, tool_calls } }
        -> 清空本次推理已使用的tool_context
        -> 发送AgentHistoryMessageWriteRequested并追加messages
        -> tool_calls为空时结束本轮
        -> tool_calls非空时合并loading_skills，加入pending_tools并派发

Tool消息：
    AgentMessage { Tool { tool_call_id, content } }
        -> 根据tool_call_id取得PendingToolCall
        -> 普通工具发送原始Tool历史事件
        -> Skill发送"skill: <scope/name> loaded"历史事件
        -> 原始Tool响应追加tool_context
        -> complete_tool_call
        -> 仍有pending_tools时等待，全部完成时发送InferenceRequest

Skill动态加载：
    Skill工具成功调用
        -> SkillPlugin按项目、AgentImage、主目录顺序读取当前SKILL.md
        -> 完整正文进入tool_context
        -> loading_skills保存Skill工具调用模板，不保存正文
        -> 每轮重新展开和调用，因此读取最新SKILL.md

记忆：
    AgentToolCallSystem -> AgentHistoryMessageWriteRequested -> MemoryPlugin历史表
    AgentContext任何变更 -> AgentContextMessagesUpdated { messages, tool_context } -> MemoryPlugin实时表
```

## 边界

```text
AgentPlugin负责：Agent创建、Message结构分支、pending_tools与loading_skills、工具来源识别、历史事件、工具派发和InferenceRequest
AgentPlugin不直接写SQLite，不解析Skill正文，不决定模型路由或Workspace生命周期
MemoryPlugin只消费历史和实时上下文事件，不读取AgentStatus或判断工具来源
ToolPlugin执行ToolCallRequest并返回AgentMessage::Tool
InferencePlugin执行InferenceRequest，发布InferenceResponse，再由自身转换System发布AgentMessage::Assistant
```

## 持有关系

```text
World
├── AgentCreateRequest Event
│   └── agent_create_system -> AgentCreated Event
├── AgentMessage Event
│   └── agent_tool_call_system
│       ├── AgentHistoryMessageWriteRequested Event
│       ├── ToolCallRequest Event
│       └── InferenceRequest Event
└── Agent Entity
    ├── AgentIdentity
    ├── AgentWorkspaceId
    ├── AgentContext
    │   ├── system_prompt
    │   ├── messages
    │   └── tool_context
    ├── AgentDefaultVisibility
    ├── AgentDynamicVisibility
    └── AgentStatus
        ├── pending_tools
        └── loading_skills
```
