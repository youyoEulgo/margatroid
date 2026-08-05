# ToolPlugin

## 类型

公开：
```text
ToolPlugin：工具调用插件，公开结构体--配置工具调用请求、异步执行响应和结果发布所属Schedule
    schedule: String--工具调用全部System所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，使用RuntimePlugin::UPDATE作为默认Schedule
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，使用schedule替换默认Schedule并返回自身
    impl Default for ToolPlugin
        Default：公开trait实现，与new等价
    impl Plugin for ToolPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装工具注册表、调用System、异步执行System和结果发布System
            行为：
                确认RuntimePlugin和AsyncRuntimePlugin已安装
                确认schedule存在且ToolRegistry尚未安装
                插入空ToolRegistry
                挂载prepare_tool_call_system
                通过add_async_system挂载ToolExecutionTask处理器
                挂载publish_tool_call_system

Tool：完整工具，公开结构体--将稳定逻辑名称、模型可见定义与实际异步执行器绑定为不可拆分的注册单元
    name: ResourceName--Margatroid内部使用的带作用域工具逻辑名称，私有
    definition: ToolDefinition--发送给模型的名称、说明和输入Schema，私有
    handler: Arc<dyn ErasedToolHandler>--类型擦除后的异步执行器，私有
    new<Arguments, Handler, HandlerFuture, Error>(
        name: ResourceName,
        definition: ToolDefinition,
        handler: Handler,
    ) -> Result<Self, ToolError>
        构造工具：公开关联函数，将公开泛型异步闭包封装为内部擦除执行器
        约束：Arguments实现DeserializeOwned + Send + 'static；Handler实现Fn(ToolContext, Arguments) -> HandlerFuture + Send + Sync + 'static；HandlerFuture实现Future<Output = Result<String, Error>> + Send + 'static；Error实现Display + Send + Sync + 'static
        行为：
            验证name、definition及input_schema
            definition.name是Provider可见调用名称，不要求等于带作用域的name
            保存definition并使用TypedToolHandler封装handler
    name(&self) -> &ResourceName
        取得逻辑名称：公开方法，返回Margatroid内部工具名称
    definition(&self) -> &ToolDefinition
        取得模型定义：公开方法，返回模型可见工具定义
    impl Clone for Tool
        Clone：公开trait实现，共享不可变执行器并克隆名称和定义

ToolContext：工具执行上下文，公开结构体--向异步工具提供请求身份和跨线程事件发送能力，不持有World
    request_id: Arc<str>--Agent本轮工具调用请求ID，私有
    agent: Entity--发起调用的AgentInstance Entity，私有
    tool_call_id: Arc<str>--Provider ToolCall ID，私有
    events: AsyncContext--异步运行时注入的事件发送上下文，私有
    request_id(&self) -> &str
        取得请求ID：公开方法，返回Agent调用请求ID
    agent(&self) -> Entity
        取得Agent：公开方法，返回发起调用的AgentInstance Entity
    tool_call_id(&self) -> &str
        取得调用ID：公开方法，返回Provider ToolCall ID
    send_event<E: Event>(&self, event: E)
        发送事件：公开方法，通过AsyncContext发送普通事件并唤醒Runtime
    send_event_after<E: Event>(&self, event: E, delay: u64)
        延迟发送：公开方法，通过AsyncContext发送延迟事件并唤醒Runtime
    impl Clone for ToolContext
        Clone：公开trait实现，共享事件发送能力并复制请求身份

AgentToolCatalog：Agent工具目录，公开组件--保存当前AgentInstance能够展示和实际调用的工具快照
    tools: BTreeMap<String, Tool>--按模型可见definition.name索引的工具，私有
    new(tools: impl IntoIterator<Item = Tool>) -> Result<Self, ToolError>
        构造目录：公开关联函数，验证并收集完整工具列表
        行为：
            同一个目录内Tool.name和ToolDefinition.name都必须唯一
            任一冲突时返回DuplicateTool或DuplicateExposedName，不生成部分目录
    definitions(&self) -> impl Iterator<Item = &ToolDefinition> + '_
        遍历定义：公开方法，按模型可见名称顺序返回当前Agent工具定义
    contains(&self, name: &ResourceName) -> bool
        检查逻辑工具：公开方法，返回目录是否包含name
    get(&self, exposed_name: &str) -> Option<&Tool>
        匹配模型调用：crate公开方法，按Provider返回的工具名称取得完整工具
    impl Component for AgentToolCatalog
        Component：公开trait实现

ToolCallCommand：工具调用命令，公开事件--请求为指定Agent匹配并执行一个完整Provider ToolCall
    id: String--Agent生成的本轮请求ID，与agent共同路由结果
    agent: Entity--发起调用的AgentInstance Entity
    call: ToolCall--InferencePlugin返回的完整工具调用
    impl Event for ToolCallCommand
        Event：公开trait实现

ToolCallResult：工具调用结果，公开事件--保留Agent和Provider调用身份并返回可写入Tool Message的文本
    id: String--原Agent请求ID
    agent: Entity--原AgentInstance Entity
    tool_call_id: String--原ToolCall ID，用于构造Message::Tool
    result: Result<String, ToolError>--成功时为工具输出文本，失败时为稳定工具错误
    impl Event for ToolCallResult
        Event：公开trait实现

ToolErrorKind：工具错误分类，公开枚举
    InvalidDefinition
    DuplicateTool
    DuplicateExposedName
    ToolPluginMissing
    ToolAlreadyRegistered
    ToolNotRegistered
    InvalidRequest
    AgentNotAlive
    ToolCatalogMissing
    ToolNotVisible
    InvalidArguments
    ExecutionFailed
    OutputLimitExceeded
    TaskPanicked

ToolError：工具错误，公开结构体--提供稳定分类和不泄露工具参数、输出或内部错误对象的有界描述
    kind: ToolErrorKind--错误分类
    message: String--有界安全描述
    new(kind: ToolErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存kind和安全描述
        行为：message超过512 UTF-8字节时在字符边界截断并追加省略号，最终长度不超过512字节
    kind(&self) -> ToolErrorKind
        取得分类：公开方法，返回kind
    message(&self) -> &str
        取得描述：公开方法，返回message引用
    panic(self) -> !
        终止：crate公开方法，使用Display文本panic消费当前错误
    impl Clone for ToolError
        Clone：公开trait实现，允许共享异步结果发布稳定错误
    impl fmt::Display for ToolError
        Display：公开trait实现，只输出kind和有界message
    impl std::error::Error for ToolError
        Error：公开trait实现

AppToolExt：App工具扩展，公开trait--供具体工具Plugin注册完整工具实现
    register_tool(&mut self, tool: Tool) -> &mut Self
        注册工具：公开方法，将完整Tool加入全局ToolRegistry
        行为：
            ToolPlugin未安装时以ToolPluginMissing终止
            相同Tool.name已经注册时以ToolAlreadyRegistered终止
            注册成功后返回App自身，注册不自动赋予任何Agent可见性

WorldToolExt：World工具扩展，公开trait--设置Agent工具快照并发送工具调用
    registered_tool(&self, name: &ResourceName) -> Option<Tool>
        取得已注册工具：公开方法，按逻辑名称从ToolRegistry克隆完整Tool；ToolPlugin未安装或name未注册时返回空
    registered_tools(
        &self,
        names: impl IntoIterator<Item = ResourceName>,
    ) -> Result<Vec<Tool>, ToolError>
        取得多个注册工具：公开方法，按names顺序克隆完整工具
        行为：ToolPlugin未安装时返回ToolPluginMissing；任一name未注册时返回ToolNotRegistered且不返回部分列表
    set_agent_tools(
        &mut self,
        agent: Entity,
        tools: impl IntoIterator<Item = Tool>,
    ) -> Result<(), ToolError>
        设置Agent工具：公开方法，原子替换AgentToolCatalog及InferencePlugin使用的AgentToolDefinitions
        行为：
            agent必须存活
            先完整构造并验证新AgentToolCatalog
            成功后才同时替换AgentToolCatalog与AgentToolDefinitions
            失败时保留旧目录和旧定义
    set_registered_agent_tools(
        &mut self,
        agent: Entity,
        names: impl IntoIterator<Item = ResourceName>,
    ) -> Result<(), ToolError>
        设置已注册工具：公开方法，按names从ToolRegistry克隆工具并调用set_agent_tools
        行为：任一name未注册时返回ToolNotRegistered，不替换Agent原目录
    send_tool_call(&self, id: impl Into<String>, agent: Entity, call: ToolCall)
        发送调用：公开方法，发送ToolCallCommand并通过Runtime封装唤醒事件驱动循环
```

crate公开：
```text
ToolRegistry：工具注册表，crate公开Resource--保存进程内所有具体工具Plugin注册的完整工具模板
    tools: BTreeMap<ResourceName, Tool>--按稳定逻辑名称索引的工具
    new() -> Self
        构造注册表：crate公开关联函数，返回空工具表
    insert(&mut self, tool: Tool) -> Result<(), ToolError>
        插入工具：crate公开方法，拒绝重复逻辑名称并保存完整工具
    get(&self, name: &ResourceName) -> Option<Tool>
        取得工具：crate公开方法，按逻辑名称克隆工具
    impl Resource for ToolRegistry
        Resource：crate公开trait实现
```

私有：
```text
ErasedToolHandler：擦除工具执行器，私有trait--统一保存不同参数类型和Future类型的工具闭包，实现Send + Sync
    call(
        &self,
        context: ToolContext,
        arguments: String,
        maximum_output_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'static>>
        调用工具：私有trait方法，解析完整JSON参数、执行具体闭包并返回有界文本

TypedToolHandler<Arguments, Handler>：类型化工具执行器，私有结构体--保留公开闭包和参数类型后实现内部擦除边界
    handler: Handler--具体工具异步闭包
    marker: PhantomData<fn() -> Arguments>--参数类型标记
    impl ErasedToolHandler for TypedToolHandler<Arguments, Handler>
        ErasedToolHandler：私有trait实现
        call(
            &self,
            context: ToolContext,
            arguments: String,
            maximum_output_bytes: usize,
        ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'static>>
            调用工具：反序列化arguments，执行handler并检查输出大小
            行为：
                arguments无法反序列化为Arguments时返回InvalidArguments
                handler返回的Error只通过Display转换为有界ExecutionFailed，不保存原错误
                成功文本超过maximum_output_bytes时返回OutputLimitExceeded，不截断为成功结果

ToolExecutionTask：工具异步执行任务，私有事件--主线程已经完成Agent目录和模型可见名称匹配
    id: String--原Agent请求ID
    agent: Entity--原AgentInstance Entity
    tool_call_id: String--原Provider ToolCall ID
    arguments: String--原完整参数JSON文本
    handler: Arc<dyn ErasedToolHandler>--从Agent工具快照克隆的执行器
    maximum_output_bytes: usize--单次工具输出上限
    impl Event for ToolExecutionTask
        Event：私有trait实现

ToolExecutionPayload：工具异步执行载荷，私有结构体--成功失败都保留原始路由
    id: String--原Agent请求ID
    agent: Entity--原AgentInstance Entity
    tool_call_id: String--原Provider ToolCall ID
    result: Result<String, ToolError>--执行结果

ToolExecutionOutput：工具异步执行输出，私有结构体--允许发布System从共享事件引用中取得一次载荷所有权
    payload: Mutex<Option<ToolExecutionPayload>>--尚未发布的执行载荷
    new(payload: ToolExecutionPayload) -> Self
        构造输出：私有关联函数，将payload保存为尚未取得的载荷
    take(&self) -> Option<ToolExecutionPayload>
        取得载荷：私有方法，从共享事件中取出一次载荷所有权；已取出时返回空

ToolTaskError：工具异步监督错误，私有结构体--表示AsyncRuntime整体取消工具处理器
    source: AsyncTaskError--异步运行时错误
    impl From<AsyncTaskError> for ToolTaskError
        From<AsyncTaskError>：私有trait实现，满足add_async_system错误约束

ToolLimits：工具调用限制，私有结构体--限制模型参数和工具成功输出进入异步任务与事件队列的大小
    maximum_arguments_bytes: usize--单次ToolCall参数JSON最大UTF-8字节数
    maximum_output_bytes: usize--单次成功输出最大UTF-8字节数
    impl Default for ToolLimits
        Default：私有trait实现，使用1MiB参数上限和4MiB输出上限

ToolState：工具执行状态，私有Resource--保存工具调用限制
    limits: ToolLimits--当前进程工具调用限制
    impl Resource for ToolState
        Resource：私有trait实现
```

## 函数

私有：
```text
prepare_tool_call_system(world: &mut World)
    准备工具调用：私有System，从Agent工具目录匹配模型可见名称并提交异步执行任务
    行为：对每个ToolCallCommand依次执行
        id、call.id或call.name为空时立即发送InvalidRequest结果
        agent不存活时立即发送AgentNotAlive结果
        agent没有AgentToolCatalog时立即发送ToolCatalogMissing结果
        call.name无法在该Agent目录中匹配时立即发送ToolNotVisible结果
        call.arguments超过maximum_arguments_bytes时立即发送InvalidArguments结果
        从匹配Tool克隆handler
        组装ToolExecutionTask并调用WorldAsyncExt::send_async_event

execute_tool(
    task: ToolExecutionTask,
    context: AsyncContext,
) -> Result<ToolExecutionOutput, ToolTaskError>
    执行工具：私有异步函数，调用擦除执行器并保留可配对路由
    行为：
        使用task和context构造ToolContext
        在panic捕获边界内调用handler.call并等待结果
        普通工具成功或失败都写入相同ToolExecutionPayload
        handler panic转换为TaskPanicked并写入相同ToolExecutionPayload
        Runtime整体取消时返回ToolTaskError

publish_tool_call_system(world: &mut World)
    发布工具结果：私有System，将异步输出转为公开ToolCallResult
    行为：
        对每个ToolExecutionOutput调用take取得一次ToolExecutionPayload
        使用相同id、agent和tool_call_id发送ToolCallResult
        ToolTaskError只在Runtime关闭等无法继续路径写system log，不伪造无法配对的业务结果

validate_tool(name: &ResourceName, definition: &ToolDefinition) -> Result<(), ToolError>
    验证工具：私有函数，检查逻辑名称和模型可见定义
    行为：
        definition.name必须非空、长度不超过64且只包含ASCII字母、数字、下划线或连字符
        description去除首尾空白后必须非空且不超过8KiB UTF-8字节
        input_schema必须是JSON对象Schema
        不尝试证明input_schema与Arguments Rust类型完全等价

bounded_message(kind: ToolErrorKind, message: impl Into<String>) -> ToolError
    构造安全错误：私有函数，调用ToolError::new限制错误文本且不记录参数或工具输出
```

## 逻辑

```text
安装ToolPlugin：
    app.add_plugin(RuntimePlugin)
        -> app.add_plugin(AsyncRuntimePlugin)
        -> app.add_plugin(ToolPlugin)
        -> 插入空ToolRegistry和ToolState
        -> 挂载prepare_tool_call_system
        -> 通过add_async_system挂载execute_tool
        -> 挂载publish_tool_call_system

扩展静态工具：
    具体工具Plugin构造Tool
        -> ResourceName标识Margatroid内部工具
        -> ToolDefinition描述模型可见名称、说明和输入Schema
        -> 异步闭包实现实际工具行为
        -> app.register_tool(tool)
        -> ToolRegistry按ResourceName保存完整Tool
    ToolPlugin自身不注册任何具体工具
    外部配置只能选择已经注册的ResourceName，不能只提供ToolDefinition创造可执行工具

创建或重载AgentInstance：
    Agent创建方取得AgentImage和Workspace确定的可见工具名称
        -> 对只有静态工具的Agent直接调用set_registered_agent_tools
        -> 需要合并动态工具时先调用registered_tools取得静态Tool
        -> 追加由SkillPlugin、McpPlugin等动态来源构造的完整Tool
        -> 使用完整列表调用set_agent_tools
        -> 先完整验证所有工具和模型可见名称唯一性
        -> 原子替换AgentToolCatalog
        -> 同步投影并替换AgentToolDefinitions
    注册表变化不自动修改已经运行的Agent工具快照
    Agent只有重载工具目录后才看到新增、替换或删除的注册工具

执行工具：
    Agent收到InferenceResult中的Assistant ToolCall
        -> 先把完整Assistant Message加入messages
        -> world.send_tool_call(request_id, agent, call)
        -> Runtime被唤醒
    prepare_tool_call_system
        -> 只在该Agent的AgentToolCatalog中按call.name匹配
        -> 不允许通过全局ToolRegistry绕过Agent可见性
        -> 验证路由和参数大小
        -> 提交ToolExecutionTask
    execute_tool
        -> 异步反序列化类型化参数
        -> 调用匹配工具闭包
        -> 工具可通过ToolContext发送其他事件，但不能直接访问World
        -> 成功、普通失败和panic都保留原调用路由
    publish_tool_call_system
        -> 发送一个ToolCallResult
        -> Agent根据agent、id和tool_call_id找到原调用
        -> 成功文本可构造Message::Tool
        -> 失败由后续Agent循环决定转换为错误Tool Message还是终止本轮

并发：
    同一Agent或不同Agent的多个ToolCallCommand分别生成独立异步任务
    AgentToolCatalog中的Tool和执行器均不可变共享
    工具结果到达顺序不保证与请求顺序一致
    Agent依靠tool_call_id关联每个结果，不依靠完成顺序

扩展边界：
    Rust具体工具Plugin通过register_tool扩展全局工具实现
    SkillPlugin和未来McpPlugin可以动态构造完整Tool并生成Agent级工具快照
    外部文件只负责可见性和工具自身配置，不直接装入任意本地执行代码
    进程外工具协议由对应适配Plugin封装，ToolPlugin不识别MCP、Shell、HTTP或Skill语义
    ToolPlugin不实现tool-call loop，不修改Agent messages，不决定工具失败后的对话策略
```

## 持有关系

```text
App
└── World
    ├── ToolRegistry Resource
    │   └── tools: BTreeMap<ResourceName, Tool>
    │       └── Tool
    │           ├── name: ResourceName
    │           ├── definition: ToolDefinition
    │           └── handler: Arc<dyn ErasedToolHandler>
    ├── ToolState Resource
    │   └── limits: ToolLimits
    └── AgentInstance Entity
        ├── AgentToolCatalog
        │   └── tools: BTreeMap<String, Tool>
        └── AgentToolDefinitions
            └── tools: Vec<ToolDefinition>

一次调用期间：
ToolCallCommand
├── id: String
├── agent: Entity
└── call: ToolCall
    -> ToolExecutionTask
       ├── id: String
       ├── agent: Entity
       ├── tool_call_id: String
       ├── arguments: String
       └── handler: Arc<dyn ErasedToolHandler>
          -> ToolContext
             ├── request_id: Arc<str>
             ├── agent: Entity
             ├── tool_call_id: Arc<str>
             └── events: AsyncContext
          -> ToolExecutionOutput
             └── Mutex<Option<ToolExecutionPayload>>
                -> ToolCallResult
```
