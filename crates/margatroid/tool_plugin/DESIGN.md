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
ToolPlugin：统一工具插件，公开结构体--安装工具定义Provider注册表
    impl Plugin for ToolPlugin
        Plugin：公开trait实现
        行为：安装ToolProviderRegistry

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
    resource(&self) -> &ResourceRef
        取得资源身份：公开方法
    definition(&self) -> &ToolDefinition
        取得模型定义：公开方法

ToolCallRequest：工具调用请求，公开事件--AgentPlugin交给ToolPlugin执行一个模型或前端指定的工具调用
    id: String--完整用户交互轮次ID
    agent: Entity--发起调用的AgentInstance Entity
    call: margatroid_types::ToolCall--需要原样执行的工具调用
    impl Event for ToolCallRequest
        Event：公开trait实现
    impl Clone for ToolCallRequest
        Clone：公开trait实现

AgentToolEnvironment：Agent工具环境，公开组件--保存工具定义和执行需要的实例位置
    project_root: Arc<PathBuf>--Workspace规范化绝对项目根，私有
    image_root: Arc<PathBuf>--AgentImage版本根，私有
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

ToolError：工具错误，公开结构体--保存稳定分类和不泄露参数、输出或路径的有界描述

AppToolExt：App工具定义扩展，公开trait
    register_tool_provider(&mut self, provider: impl ToolDefinitionProvider) -> &mut Self
        注册Provider：公开方法，按provider.id加入ToolProviderRegistry并拒绝重复ID
    register_tool(&mut self, tool: Tool) -> &mut Self
        注册普通Tool：公开方法，加入内置provider="tool"的精确名称表

WorldToolExt：World工具扩展，公开trait
    resolve_tool(
        &self,
        agent: Entity,
        resource: &ResourceRef,
    ) -> Result<Tool, ToolError>
        构造工具：公开方法，为单个ResourceRef构造单个Tool
```

私有：
```text
ToolProviderRegistry：工具定义Provider注册表，私有Resource
    providers: BTreeMap<String, Arc<dyn ToolDefinitionProvider>>--Provider ID到实现
    static_tools: BTreeMap<ResourceName, Tool>--内置tool Provider的精确普通工具定义

ErasedToolHandler：擦除工具执行器，私有trait
```

## 函数

```text
resolve_tool(world: &World, agent: Entity, resource: &ResourceRef)
    解析单个工具：
        验证Agent存活并读取AgentToolEnvironment
        按resource.provider找到唯一ToolDefinitionProvider
        调用provider.provide(environment, resource.name)构造Tool
        验证Tool.resource等于输入resource
        返回Tool
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
    AgentPlugin发送ToolCallRequest
        -> ToolPlugin接收单个调用并进入工具调用处理
        -> 成功或失败都构造Message::Tool
        -> 发送margatroid_types::AgentMessage { id, agent, message, intent: ResolveToolCall }

边界：
    AgentPlugin拥有并遍历AgentDynamicVisibility.resources
    ToolPlugin一次只知道当前ResourceRef，不接收资源集合
    ToolProviderRegistry只说明一个资源如何变成Tool，不决定请求中有哪些工具
    ToolPlugin只执行AgentPlugin派发的ToolCallRequest，不读取Agent可见性组件，也不做可见性检查
    ToolCallRequest之后的工具定位与执行步骤后续定义
```
