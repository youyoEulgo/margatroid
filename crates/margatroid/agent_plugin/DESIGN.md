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

## 类型

公开：
```text
AgentPlugin：Agent实例与消息循环插件，公开结构体--安装创建和消息处理System
    schedule: String--两个System所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，默认使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，替换默认Schedule并返回自身
    impl Default for AgentPlugin
        Default：公开trait实现，与new等价
        default() -> Self
            构造默认插件：调用new
    impl Plugin for AgentPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：要求RuntimePlugin和目标Schedule存在
            行为：依次挂载agent_create_system和agent_message_system

AgentCreateRequest：Agent创建请求，公开事件--WorkspacePlugin交付创建实例所需的Agent自有字段
    id: String--Workspace生成的创建子请求ID
    agent_id: String--Workspace生成的稳定Agent逻辑ID，例如demo.coder0
    workspace_id: Entity--Agent所属Workspace Entity
    system_prompt: String--当前系统提示词
    messages: Vec<margatroid_types::Message>--恢复出的动态上下文，不包含System Message
    default_visibility: BTreeSet<margatroid_types::ResourceRef>--创建时默认可见资源
    impl Event for AgentCreateRequest
        Event：公开trait实现
    impl Clone for AgentCreateRequest
        Clone：公开trait实现

AgentCreated：Agent创建完成回执，公开事件--把创建子请求与新Entity关联
    id: String--原创建子请求ID
    agent: Entity--新Agent Entity
    impl Event for AgentCreated
        Event：公开trait实现
    impl Clone for AgentCreated
        Clone：公开trait实现

AgentIdentity：Agent稳定身份，公开组件--保存跨Plugin和跨进程使用的逻辑Agent ID
    id: String--唯一Agent ID，例如demo.coder0，私有
    id(&self) -> &str
        取得Agent ID：公开方法，返回稳定逻辑ID
    impl Component for AgentIdentity
        Component：公开trait实现

AgentWorkspaceId：Agent所属Workspace标识，公开组件
    workspace_id: Entity--Workspace Entity，私有
    workspace_id(&self) -> Entity
        取得Workspace：公开方法
    impl Component for AgentWorkspaceId
        Component：公开trait实现

AgentContext：Agent上下文，公开组件--分开保存系统提示词与动态消息
    system_prompt: String--当前系统提示词，私有
    messages: Vec<margatroid_types::Message>--User、Assistant和Tool动态消息，私有
    system_prompt(&self) -> &str
        取得系统提示词：公开只读方法
    messages(&self) -> &[margatroid_types::Message]
        取得动态上下文：公开只读方法
    append_message(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender)
        追加消息：公开方法，拒绝System Message并追加一条动态消息
        行为：修改后发送AgentContextMessagesUpdated完整快照
    rewrite_messages(&mut self, agent: Entity, messages: Vec<Message>, events: &RuntimeEventSender)
        重写消息：公开方法，拒绝System Message并整体替换动态上下文
        行为：修改后发送AgentContextMessagesUpdated完整快照
    限制：append_message和rewrite_messages是创建完成后仅有的可变入口
    impl Component for AgentContext
        Component：公开trait实现

AgentDefaultVisibility：Agent默认可见性，公开组件--创建时确定后只读
    resources: BTreeSet<ResourceRef>--默认可见资源，私有
    resources(&self) -> &BTreeSet<ResourceRef>
        取得默认可见性：公开只读方法
    impl Component for AgentDefaultVisibility
        Component：公开trait实现

AgentDynamicVisibility：Agent动态可见性，公开组件--当前实际可见资源
    resources: BTreeSet<ResourceRef>--动态可见资源，私有
    resources(&self) -> &BTreeSet<ResourceRef>
        取得动态可见性：公开只读方法
    impl Component for AgentDynamicVisibility
        Component：公开trait实现

AgentStatus：Agent轮次状态，公开组件--当前只跟踪一个工具调用批次
    pending_tools: Option<PendingToolCalls>--等待响应的当前批次，私有
    is_waiting_for_tools(&self) -> bool
        是否等待工具：公开只读方法
    pending_turn_id(&self) -> Option<&str>
        等待轮次：公开只读方法
    pending_tool_call_ids(&self) -> impl Iterator<Item = &str> + '_
        等待调用：公开只读方法
    begin_tool_calls(&mut self, id: &str, tool_calls: &[ToolCall]) -> Result<(), AgentStepError>
        开始工具批次：私有方法，要求当前没有批次、轮次ID非空、调用非空且ToolCall ID非空不重复
        行为：一次性保存轮次ID和全部待完成ToolCall ID
    accepts_tool_response(&self, id: &str, tool_call_id: &str) -> bool
        验证工具响应：私有只读方法，仅当轮次和ToolCall ID属于当前待完成批次时返回true
    complete_tool_response(&mut self, id: &str, tool_call_id: &str) -> bool
        完成工具响应：私有方法，删除匹配的待完成ID；集合清空时清除整个批次并返回true
    impl Component for AgentStatus
        Component：公开trait实现
```

私有：
```text
AgentPluginInstalled：AgentPlugin安装标记，公开单元Resource--阻止同一App重复安装并供WorkspacePlugin确认依赖
    impl Resource for AgentPluginInstalled
        Resource：公开trait实现

PendingToolCalls：待完成工具批次，私有结构体
    id: String--完整交互轮次ID
    tool_call_ids: BTreeSet<String>--尚未返回的唯一ToolCall ID

AvailableTools：一次请求的临时工具集合，私有结构体--不是Agent组件
    definitions: Vec<ToolDefinition>--发给InferencePlugin的工具定义
    resources_by_name: BTreeMap<String, ResourceRef>--模型可见名称到资源引用的临时映射

AgentStepError：当前事件处理错误，私有枚举--Memory错误单独上报，其余错误转换为AgentFailure
    Memory(MemoryError)--历史写入失败
    AgentMissing--Agent Entity不存在
    ContextMissing--AgentContext或动态可见性不存在
    StatusMissing--AgentStatus不存在
    InvalidMessage--Message与Intent组合非法
    InvalidToolBatch--工具批次为空、冲突、重复或响应不属于当前批次
    Tool(ToolError)--工具定义解析、请求或执行准备错误
    DuplicateToolName--两个可见资源暴露同一个模型工具名
    failure_message(&self) -> String
        构造失败描述：返回不包含消息正文、工具参数或资源正文的稳定有界文本
```

## 函数

私有：
```text
agent_create_system(world: &mut World)
    创建Agent：读取AgentCreateRequest并创建Agent Entity
    行为：
        收集当前全部请求
        要求id、agent_id非空、workspace_id存活且messages不包含System
        创建Entity并插入AgentIdentity、AgentWorkspaceId、AgentContext、两层Visibility和AgentStatus
        AgentDynamicVisibility初始完整复制AgentDefaultVisibility
        发送AgentCreated { id, agent }
        不读取Workspace、AgentImage或磁盘，不挂载其他Plugin拥有的组件

agent_message_system(world: &mut World)
    处理Agent消息：读取统一AgentMessage并使用其中的Entity调用handle_message
    行为：
        Agent Entity不存在时记录warning并结束当前事件
        Memory错误发送AgentMemoryWriteFailed
        其他错误发送AgentFailure { kind: Agent, id, agent, message }
        失败不伪造Assistant或Tool消息，不继续当前事件后续步骤

handle_message(world: &mut World, agent: Entity, event: &AgentMessage, events: &RuntimeEventSender)
    处理单条消息：使用已解析Entity，先验证MessageIntent与Message类型组合，再记录消息并进入分支
    行为：
        UserWithToolCalls只接受User，记录后派发指定工具批次
        UserWithoutToolCalls只接受User，记录后发送InferenceCommand
        DispatchToolCalls只接受Assistant，记录后派发Assistant.tool_calls
        ResolveToolCall只接受Tool，确认属于当前批次后记录；最后一个响应才发送InferenceCommand
        CompleteTurn只接受Assistant，记录后结束
        不根据消息正文推断Intent

record_message(world: &mut World, agent: Entity, event: &AgentMessage, events: &RuntimeEventSender)
    记录消息：同步历史表并更新AgentContext
    行为：
        User和Assistant先调用WorldMemoryExt::append_history_message
        历史提交失败时不修改AgentContext
        Tool跳过历史写入
        成功后调用AgentContext.append_message

build_available_tools(world: &World, agent: Entity) -> Result<AvailableTools, AgentStepError>
    构造可用工具：每次直接从AgentDynamicVisibility构造请求工具与名称映射
    行为：
        按BTreeSet顺序遍历每个ResourceRef
        逐个调用WorldToolExt::resolve_tool
        收集ToolDefinition
        建立definition.name到ResourceRef的临时映射
        definition.name重复时整体失败，不产生歧义映射
        不把AvailableTools保存为Component

dispatch_tool_calls(
    world: &mut World,
    id: &str,
    agent: Entity,
    tool_calls: &[ToolCall],
    events: &RuntimeEventSender,
)
    派发工具：把模型名称解析成ResourceRef并启动一个待完成批次
    行为：
        调用build_available_tools
        每个ToolCall.name必须出现在临时映射
        ToolCall ID必须非空且批次内唯一
        AgentStatus不能已经等待另一个批次
        全部请求构造成功后一次性写入AgentStatus
        逐个发送ToolCallRequest { id, agent, resource, call }

send_inference_command(world: &World, id: &str, agent: Entity, events: &RuntimeEventSender)
    发送推理：使用当前系统提示词、动态消息和动态可见性构造请求
    行为：
        调用build_available_tools
        第一条Message由AgentContext.system_prompt构造为System
        后续完整复制AgentContext.messages
        读取AgentIdentity.id
        发送InferenceCommand { id, agent, agent_id, messages, tools }

validate_message_intent(event: &AgentMessage)
    验证消息契约：只检查来源赋予的Intent与静态Message类型是否匹配，不重新决定Intent

assert_dynamic_message(message: &Message)
    断言动态消息：私有函数，message为System时panic

assert_dynamic_messages(messages: &[Message])
    断言动态消息集合：私有函数，逐条调用assert_dynamic_message
```

## 逻辑

```text
创建：
    WorkspacePlugin发送AgentCreateRequest { id, agent_id, workspace_id, system_prompt, messages, default_visibility }
        -> agent_create_system创建Agent自有组件
        -> 插入AgentIdentity { id: agent_id }
        -> 发送AgentCreated { id, agent }
        -> WorkspacePlugin按id取回PreparedWorkspaceAgent
        -> WorkspacePlugin绑定AgentMemory、AgentInferenceSnapshot和AgentToolEnvironment

普通用户消息：
    AgentMessage { agent: Entity, User, UserWithoutToolCalls }
        -> AgentPlugin直接使用事件携带的Agent Entity
        -> AgentIdentity只用于构造InferenceCommand和日志中的稳定ID
        -> history_messages追加User
        -> AgentContext追加User并通知MemoryPlugin重写realtime_messages
        -> 当前AgentDynamicVisibility逐项构造ToolDefinition
        -> 发送InferenceCommand

工具批次：
    UserWithToolCalls或DispatchToolCalls
        -> 记录User或Assistant消息
        -> 构造临时exposed name到ResourceRef映射
        -> AgentStatus记录全部待完成ToolCall ID
        -> 逐个发送ToolCallRequest

    每个Tool响应发送AgentMessage { agent: Entity(agent), Tool, ResolveToolCall }
        -> 验证turn id与tool_call_id属于当前批次
        -> Tool只进入AgentContext和realtime_messages，不进入history_messages
        -> 从AgentStatus移除tool_call_id
        -> 尚有待完成调用时结束当前事件
        -> 最后一个响应到达时清空批次并只发送一次InferenceCommand

最终响应：
    AgentMessage { Assistant, CompleteTurn }
        -> history_messages追加Assistant
        -> AgentContext追加Assistant
        -> 结束轮次

边界：
    AgentPlugin只持有WorkspaceId、Context、两层Visibility和Status
    AgentPlugin不持有工具快照
    InferenceCommand.tools的唯一集合来源是AgentDynamicVisibility
    ToolPlugin收到的是单个已经解析的ResourceRef，不接收可见性集合，也不检查可见性权限
    AgentPlugin不决定上下文压缩、资源正文解析、模型路由或Workspace创建、重载和关闭
```

## 持有关系

```text
World
├── AgentCreateRequest Event
│   └── agent_create_system
│       └── AgentCreated Event
├── margatroid_types::AgentMessage Event
│   └── agent_message_system
│       ├── memory_plugin::WorldMemoryExt
│       ├── tool_plugin::ToolCallRequest Event
│       └── inference_plugin::InferenceCommand Event
└── Agent Entity
    ├── AgentWorkspaceId
    ├── AgentContext
    ├── AgentDefaultVisibility
    ├── AgentDynamicVisibility
    └── AgentStatus
```
