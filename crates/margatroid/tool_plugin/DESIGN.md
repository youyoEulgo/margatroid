# ToolPlugin

## 定位

ToolPlugin统一处理所有可被模型调用的资源。普通Rust Tool、Skill、Workflow和未来资源都由
`ToolDefinitionProvider`提供，并通过同一条构造链路进入模型请求。

ToolPlugin每次只借用一个`ResourceRef`并返回一个`Tool`。哪些`ResourceRef`进入模型请求由
AgentPlugin遍历`AgentDynamicVisibility.resources`决定；ToolPlugin不接收该集合，也不读取任何
Agent可见性组件。

## 类型

公开：
```text
ToolPlugin：统一工具插件，公开结构体--安装Provider注册表和工具执行System
    schedule: String--工具准备、异步执行和结果发布所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，默认使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，替换默认Schedule并返回自身
    impl Default for ToolPlugin
        Default：公开trait实现，与new等价
        default() -> Self
            构造默认插件：调用new
    impl Plugin for ToolPlugin
        Plugin：公开trait实现
        行为：要求RuntimePlugin和AsyncRuntimePlugin，安装ToolPluginInstalled、ToolProviderRegistry与ToolState并挂载三个System

ToolPluginInstalled：工具插件安装标记，公开单元Resource--供WorkspacePlugin确认依赖并阻止重复安装
    impl Resource for ToolPluginInstalled
        Resource：公开trait实现

ToolDefinitionProvider：工具定义提供方，公开trait--把一个ResourceRef解析为完整Tool
    id(&self) -> &str
        Provider ID：公开方法，必须与ResourceRef.provider稳定对应
    provide(
        &self,
        environment: &AgentToolEnvironment,
        name: &ResourceName,
    ) -> Result<Tool, ToolError>
        提供工具：公开trait方法，为指定Agent位置和资源名构造完整Tool
        限制：只构造definition和handler，不改变Agent组件
    约束：Send + Sync + 'static

Tool：一个可发送给模型并可执行的完整工具，公开结构体
    resource: ResourceRef--产生该Tool的资源身份，私有
    definition: ToolDefinition--发送给模型的名称、说明和输入Schema，私有
    handler: Arc<dyn ErasedToolHandler>--异步执行器，私有
    new<Arguments, Handler, HandlerFuture, Error>(resource: ResourceRef, definition: ToolDefinition, handler: Handler) -> Result<Self, ToolError>
        构造工具：公开泛型关联函数，验证资源和定义并用TypedToolHandler擦除参数与Future类型
        约束：Arguments可反序列化且可发送；Handler和Future可发送；Error可显示
    resource(&self) -> &ResourceRef
        取得资源身份：公开方法
    definition(&self) -> &ToolDefinition
        取得模型定义：公开方法

ToolContext：工具执行上下文，公开结构体--向handler提供请求身份、Agent和事件发送能力
    request_id: Arc<str>--完整交互轮次ID，私有
    agent: Entity--发起调用的Agent，私有
    tool_call_id: Arc<str>--本次ToolCall ID，私有
    events: AsyncContext--异步事件发送上下文，私有
    request_id(&self) -> &str
        取得请求ID：公开方法
    agent(&self) -> Entity
        取得Agent：公开方法
    tool_call_id(&self) -> &str
        取得ToolCall ID：公开方法
    send_event<E: Event>(&self, event: E)
        发送事件：公开泛型方法，立即发送并唤醒Runtime
    send_event_after<E: Event>(&self, event: E, delay: u64)
        延迟事件：公开泛型方法，按Core tick延迟发送

ToolCallRequest：工具调用请求，公开事件--AgentPlugin交给ToolPlugin执行一个模型或前端指定的工具调用
    id: String--完整用户交互轮次ID
    agent: Entity--发起调用的AgentInstance Entity
    resource: ResourceRef--AgentPlugin按当次工具名称映射得到的唯一资源身份
    call: margatroid_types::ToolCall--需要原样执行的工具调用
    impl Event for ToolCallRequest
        Event：公开trait实现
    impl Clone for ToolCallRequest
        Clone：公开trait实现

AgentToolEnvironment：Agent工具环境，公开组件--保存工具定义和执行需要的实例位置
    project_root: Arc<PathBuf>--Workspace规范化绝对项目根，私有
    image_root: Arc<PathBuf>--AgentImage版本根，私有
    new(project_root: impl Into<PathBuf>, image_root: impl Into<PathBuf>) -> Self
        构造环境：公开关联函数，由WorkspacePlugin传入已经确定的实例位置
    project_root(&self) -> &Path
        取得项目根：公开方法
    image_root(&self) -> &Path
        取得镜像根：公开方法
    impl Component for AgentToolEnvironment
        Component：公开trait实现

ToolErrorKind：工具错误分类，公开枚举
    InvalidDefinition
    DuplicateProvider
    ProviderMissing
    ResourceResolutionFailed
    AgentNotAlive
    ToolEnvironmentMissing
    ToolPluginMissing
    ToolAlreadyRegistered
    InvalidRequest
    InvalidArguments
    ExecutionFailed
    OutputLimitExceeded
    TaskPanicked

ToolError：工具错误，公开结构体--保存稳定分类和不泄露参数、输出或路径的有界描述
    kind: ToolErrorKind--错误分类，私有
    message: String--有界稳定描述，私有
    new(kind: ToolErrorKind, message: impl Into<String>) -> Self
        构造错误：公开关联函数，截断超过512字节的描述
    kind(&self) -> ToolErrorKind
        取得分类：公开方法
    message(&self) -> &str
        取得描述：公开方法
    panic(self) -> !
        终止配置：crate私有方法，以Display文本触发panic
    impl fmt::Display for ToolError
        Display：公开trait实现，输出分类和描述
    impl std::error::Error for ToolError
        Error：公开trait实现

AppToolExt：App工具定义扩展，公开trait
    register_tool_provider(&mut self, provider: impl ToolDefinitionProvider) -> &mut Self
        注册Provider：公开方法，按provider.id加入ToolProviderRegistry并拒绝重复ID
    register_tool(&mut self, tool: Tool) -> &mut Self
        注册普通Tool：公开方法，加入内置provider="tool"的精确名称表

WorldToolExt：World工具扩展，公开trait
    registered_tool(&self, name: &ResourceName) -> Option<Tool>
        查询静态工具：公开方法，按内置provider的ResourceName返回Tool克隆
    resolve_tool(
        &self,
        agent: Entity,
        resource: &ResourceRef,
    ) -> Result<Tool, ToolError>
        构造工具：公开方法，为单个ResourceRef构造单个Tool
        行为：验证Agent和AgentToolEnvironment，按provider取得定义，要求返回资源与输入完全相同
    impl WorldToolExt for World
        World工具扩展：公开trait实现
```

私有：
```text
ToolProviderRegistry：工具定义Provider注册表，私有Resource
    providers: BTreeMap<String, Arc<dyn ToolDefinitionProvider>>--Provider ID到实现
    static_tools: BTreeMap<ResourceName, Tool>--内置tool Provider的精确普通工具定义
    new() -> Self
        构造注册表：私有关联函数，创建两个空映射
    insert_provider<P: ToolDefinitionProvider>(&mut self, provider: P) -> Result<(), ToolError>
        插入Provider：私有泛型方法，验证ID并拒绝重复或保留ID tool
    insert_static_tool(&mut self, tool: Tool) -> Result<(), ToolError>
        插入静态工具：私有方法，按资源名称拒绝重复
    impl Resource for ToolProviderRegistry
        Resource：私有trait实现

ErasedToolHandler：擦除工具执行器，私有trait
    call(&self, context: ToolContext, arguments: String, maximum_output_bytes: usize) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'static>>
        执行擦除工具：私有方法，返回类型擦除Future

TypedToolHandler<Arguments, Handler>：类型化工具执行器，私有结构体--保存handler并在call时反序列化参数
    handler: Handler--用户注册处理器
    marker: PhantomData<fn() -> Arguments>--参数类型标记
    impl ErasedToolHandler for TypedToolHandler
        call：参数JSON不匹配时返回InvalidArguments；业务错误转ExecutionFailed；输出超限返回OutputLimitExceeded

ToolState：工具执行限制，私有Resource
    maximum_arguments_bytes: usize--工具参数最大字节数
    maximum_output_bytes: usize--工具输出最大字节数
    impl Default for ToolState
        Default：私有trait实现，使用1MiB参数和4MiB输出上限
        default() -> Self
            构造默认限制：返回固定限制

ToolExecutionTask：工具异步执行请求，私有事件
    id: String
    agent: Entity
    tool_call_id: String
    handler: Arc<dyn ErasedToolHandler>
    arguments: String
    maximum_output_bytes: usize
    impl Event for ToolExecutionTask
        Event：私有trait实现

ToolExecutionOutput：工具异步执行结果，私有结构体
    id: String
    agent: Entity
    tool_call_id: String
    result: Result<String, ToolError>

ToolTaskError：异步基础设施错误包装，私有元组结构体
    0: AsyncTaskError
    impl From<AsyncTaskError> for ToolTaskError
        转换任务错误：私有trait实现
        from(error: AsyncTaskError) -> Self
            转换监督错误：包装为元组字段
```

## 函数

```text
prepare_tool_call_system(world: &mut World)
    准备调用：消费ToolCallRequest
        验证请求字段和参数长度
        只调用resolve_tool(agent, &request.resource)，不读取可见性组件
        验证Tool.definition.name等于ToolCall.name
        发送AgentResourcesUsed { id, agent, resources: [request.resource] }
        把handler和参数包装为ToolExecutionTask交给AsyncRuntime
        准备失败时直接发送错误内容的Tool AgentMessage

execute_tool(task: ToolExecutionTask, context: AsyncContext) -> Result<ToolExecutionOutput, ToolTaskError>
    执行工具：反序列化参数、调用异步handler并限制输出大小
        handler panic转换为TaskPanicked
        成功和业务失败都保留id、agent与tool_call_id

publish_tool_call_system(world: &mut World)
    发布结果：把ToolExecutionOutput转换为AgentMessage
        成功时Message::Tool.content使用完整工具输出
        失败时content使用有界ToolError文本
        intent固定为ResolveToolCall

send_tool_message(world: &World, id: String, agent: Entity, tool_call_id: String, content: String)
    发送工具消息：私有函数，构造Message::Tool与ResolveToolCall意图的AgentMessage

validate_tool(resource: &ResourceRef, definition: &ToolDefinition) -> Result<(), ToolError>
    验证工具：私有函数，校验模型名称、描述、JSON object Schema和资源身份

is_valid_provider_id(id: &str) -> bool
    验证Provider ID：私有函数，只接受非空小写ASCII字母、数字、下划线和短横线
```

## 逻辑

```text
注册定义Provider：
    普通Rust Tool  -> 内置provider="tool"
    SkillPlugin    -> provider="skill"
    WorkflowPlugin -> provider="workflow"
    未来Plugin     -> 自己的稳定provider ID

每次LLM请求：
    AgentPlugin读取AgentDynamicVisibility.resources
        -> AgentPlugin逐个遍历ResourceRef
        -> 每次调用ToolPlugin.resolve_tool(agent, &resource)
        -> AgentPlugin收集Tool.definition
        -> 写入InferenceCommand.tools

处理工具调用：
    AgentPlugin发送ToolCallRequest { id, agent, resource, call }
        -> ToolPlugin只解析request.resource并执行对应Tool
        -> ToolPlugin不重新从ToolCall.name猜测ResourceRef
        -> 实际解析成功后发送AgentResourcesUsed
        -> 成功或失败都构造Message::Tool
        -> 发送margatroid_types::AgentMessage { id, agent, message, intent: ResolveToolCall }

边界：
    AgentPlugin拥有并遍历AgentDynamicVisibility.resources
    ToolPlugin一次只知道当前ResourceRef，不接收资源集合
    ToolProviderRegistry只说明一个资源如何变成Tool，不决定请求中有哪些工具
    ToolPlugin只执行AgentPlugin派发的ToolCallRequest，不读取Agent可见性组件，也不做可见性检查
    ToolCall.name只用于确认模型可见名称与已解析Tool一致，不用于可见性判断
```
