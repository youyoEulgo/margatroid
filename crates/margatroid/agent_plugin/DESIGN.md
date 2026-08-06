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
AgentCreateRequest：Agent创建请求，公开结构体--WorkspacePlugin提供创建Agent实例所需的全部信息
    workspace_id: Entity--Agent所属Workspace Entity，公开
    system_prompt: String--Agent系统提示词，公开
    messages: Vec<margatroid_types::Message>--Agent初始动态消息，不包含System Message，公开
    default_visibility: BTreeSet<margatroid_types::ResourceRef>--Agent默认可见资源，公开
    impl Event for AgentCreateRequest
        Event：公开trait实现
    impl Clone for AgentCreateRequest
        Clone：公开trait实现

AgentWorkspaceId：Agent所属Workspace标识，公开结构体--WorkspacePlugin在创建Agent时提供
    workspace_id: Entity--Workspace Entity，私有
    impl Component for AgentWorkspaceId
        Component：公开trait实现

AgentContext：Agent上下文，公开结构体--分开保存当前系统提示词与动态消息
    system_prompt: String--当前系统提示词，私有
    messages: Vec<margatroid_types::Message>--动态消息，不包含System Message，私有
    append_message(
        &mut self,
        agent: Entity,
        message: margatroid_types::Message,
        events: &app_runtime_plugin::RuntimeEventSender,
    )
        追加消息：公开方法，将message追加到messages末尾
        行为：追加完成后发送margatroid_types::AgentContextMessagesUpdated { agent, messages: self.messages.clone() }
    rewrite_messages(
        &mut self,
        agent: Entity,
        messages: Vec<margatroid_types::Message>,
        events: &app_runtime_plugin::RuntimeEventSender,
    )
        重写消息：公开方法，使用传入的messages整体替换当前messages
        行为：替换完成后发送margatroid_types::AgentContextMessagesUpdated { agent, messages: self.messages.clone() }
    限制：创建完成后不公开messages的可变引用；append_message和rewrite_messages是仅有的修改入口
    impl Component for AgentContext
        Component：公开trait实现

AgentDefaultVisibility：Agent默认可见性，公开结构体--Workspace创建Agent时确定的只读集合
    resources: BTreeSet<margatroid_types::ResourceRef>--默认可见资源，私有
    impl Component for AgentDefaultVisibility
        Component：公开trait实现

AgentDynamicVisibility：Agent动态可见性，公开结构体--Agent运行中实际可见的资源集合
    resources: BTreeSet<margatroid_types::ResourceRef>--当前可见资源，私有
    impl Component for AgentDynamicVisibility
        Component：公开trait实现

AgentStatus：Agent状态，公开单元结构体--当前只保留状态组件，具体状态后续定义
    impl Component for AgentStatus
        Component：公开trait实现
```

## 函数

私有：
```text
record_message(
    world: &mut World,
    event: &margatroid_types::AgentMessage,
    events: &app_runtime_plugin::RuntimeEventSender,
) -> Result<(), memory_plugin::MemoryError>
    记录消息：私有函数，按Message类型决定是否写入历史表，并始终追加到AgentContext
    行为：
        event.message是Message::User或Message::Assistant时：
            调用memory_plugin::WorldMemoryExt::append_history_message(world, event)
            历史写入失败时不修改AgentContext并返回MemoryError
        event.message是Message::Tool时不调用append_history_message
        读取event.agent的AgentContext
        调用AgentContext.append_message(event.agent, event.message.clone(), events)
        append_message发送margatroid_types::AgentContextMessagesUpdated

build_tool_definitions(
    world: &World,
    agent: Entity,
    resources: &BTreeSet<margatroid_types::ResourceRef>,
) -> Result<Vec<margatroid_types::ToolDefinition>, tool_plugin::ToolError>
    构造工具定义：私有函数，将动态可见资源逐个转换为模型工具定义
    行为：
        创建空tools列表
        依次遍历resources中的每个resource
        调用tool_plugin::WorldToolExt::resolve_tool(world, agent, resource)
        成功时克隆tool.definition()并追加到tools
        任一资源构造失败时返回ToolError，不返回部分tools
        全部成功时返回tools

dispatch_tool_calls(
    events: &app_runtime_plugin::RuntimeEventSender,
    id: &str,
    agent: Entity,
    tool_calls: &[margatroid_types::ToolCall],
)
    派发工具调用：私有函数，将一组工具调用逐个包装为ToolPlugin请求
    行为：
        依次遍历tool_calls
        为每个调用发送tool_plugin::ToolCallRequest {
            id: id.to_owned(),
            agent,
            call: tool_call.clone(),
        }

agent_create_system(world: &mut World)
    创建Agent：私有System，读取AgentCreateRequest并创建Agent Entity
    行为：
        收集本次全部AgentCreateRequest，结束对EventReader的借用
        按事件顺序处理每个AgentCreateRequest
        调用World::spawn创建Agent Entity
        使用request.workspace_id构造AgentWorkspaceId并插入Entity
        使用request.system_prompt和request.messages构造AgentContext并插入Entity
        克隆request.default_visibility作为dynamic_visibility
        使用request.default_visibility构造AgentDefaultVisibility并插入Entity
        使用dynamic_visibility构造AgentDynamicVisibility并插入Entity
        构造AgentStatus并插入Entity
        不读取AgentImage、Workspace Entity组件或磁盘内容

agent_message_system(world: &mut World)
    处理Agent消息：私有System，记录全部AgentMessage并按intent进入对应分支
    行为：
        收集本次全部margatroid_types::AgentMessage，结束对EventReader的借用
        调用WorldEventExt::event_sender取得RuntimeEventSender
        按事件顺序处理每个margatroid_types::AgentMessage
        调用record_message(world, &event, &events)
            失败时发送memory_plugin::AgentMemoryWriteFailed并结束当前事件
        根据event.intent进入分支：
            margatroid_types::MessageIntent::CompleteTurn
                直接结束当前事件，不发送工具调用或新的InferenceCommand
            margatroid_types::MessageIntent::UserWithToolCalls { tool_calls }
                调用dispatch_tool_calls(&events, &event.id, event.agent, &tool_calls)
                结束当前事件，不发送InferenceCommand
            margatroid_types::MessageIntent::DispatchToolCalls
                event.message必须是Message::Assistant
                从event.message读取Assistant.tool_calls
                调用dispatch_tool_calls(&events, &event.id, event.agent, &tool_calls)
                结束当前事件，不发送InferenceCommand
            margatroid_types::MessageIntent::UserWithoutToolCalls
                继续构造InferenceCommand
            margatroid_types::MessageIntent::ResolveToolCall
                Tool响应已经由record_message追加到AgentContext，但未写入历史表
                继续构造InferenceCommand
        读取event.agent的AgentDynamicVisibility
        调用build_tool_definitions(world, event.agent, &AgentDynamicVisibility.resources)
            失败时结束当前事件，错误传递方式后续定义
            成功时取得tools
        结束对AgentDynamicVisibility的只读借用
        读取event.agent的AgentContext
        构造messages：
            第一条是使用AgentContext.system_prompt构造的margatroid_types::Message::System
            后续依次克隆AgentContext.messages
        结束对AgentContext的只读借用
        发送inference_plugin::InferenceCommand {
            id: event.id,
            agent: event.agent,
            messages,
            tools,
            stream: None
        }
        失败处理暂不定义
```

## 逻辑

```text
创建Agent：
    WorkspacePlugin收集完整创建信息
        -> 发送AgentCreateRequest
        -> agent_create_system创建Entity
        -> 插入AgentWorkspaceId、AgentContext、AgentDefaultVisibility、AgentDynamicVisibility和AgentStatus

处理消息：
    任意消息来源发送AgentMessage
        -> agent_message_system调用record_message
        -> User或Assistant消息由record_message调用MemoryPlugin追加到history_messages
        -> Tool消息跳过history_messages
        -> record_message调用AgentContext.append_message追加event.message
        -> AgentContext发送margatroid_types::AgentContextMessagesUpdated
        -> MemoryPlugin重写realtime_messages
        -> agent_message_system根据event.intent进入分支

处理没有前端指定工具调用的用户消息分支：
    MessageIntent::UserWithoutToolCalls
        -> 使用AgentContext构造InferenceCommand.messages
        -> 调用build_tool_definitions按AgentDynamicVisibility构造InferenceCommand.tools
        -> 发送InferenceCommand

处理带有前端指定工具调用的用户消息分支：
    MessageIntent::UserWithToolCalls { tool_calls }
        -> 调用dispatch_tool_calls逐个发送tool_plugin::ToolCallRequest
        -> 当前事件不发送InferenceCommand
        -> ToolPlugin完成调用后发送AgentMessage { Message::Tool, intent: ResolveToolCall }

处理模型返回工具调用分支：
    InferencePlugin发送AgentMessage { Message::Assistant, intent: DispatchToolCalls }
        -> AgentPlugin记录Assistant消息
        -> 从Assistant.tool_calls取得调用列表
        -> 调用dispatch_tool_calls逐个发送tool_plugin::ToolCallRequest
        -> 当前事件不发送InferenceCommand
        -> ToolPlugin完成调用后发送AgentMessage { Message::Tool, intent: ResolveToolCall }

处理工具响应分支：
    MessageIntent::ResolveToolCall
        -> Tool消息只写入AgentContext和realtime_messages，不写入history_messages
        -> 使用更新后的AgentContext构造InferenceCommand.messages
        -> 调用build_tool_definitions按AgentDynamicVisibility构造InferenceCommand.tools
        -> 发送InferenceCommand

处理无工具调用响应分支：
    InferencePlugin发送AgentMessage { intent: CompleteTurn }
        -> 历史消息和AgentContext更新完成
        -> 不检查message.tool_calls
        -> 不发送任何后续事件，直接结束当前轮次

InferenceCommand中的tools：
    每次发送InferenceCommand
        -> 都由build_tool_definitions读取当前AgentDynamicVisibility构造
        -> 与用户消息是否携带前端指定的tool_calls无关
        -> 动态可见性为空时tools才自然为空
```

## 持有关系

```text
World
├── AgentCreateRequest Event
├── margatroid_types::AgentMessage Event
├── tool_plugin::ToolCallRequest Event
├── inference_plugin::InferenceCommand Event
└── Agent Entity
    ├── AgentWorkspaceId
    ├── AgentContext
    ├── AgentDefaultVisibility
    ├── AgentDynamicVisibility
    └── AgentStatus
```
