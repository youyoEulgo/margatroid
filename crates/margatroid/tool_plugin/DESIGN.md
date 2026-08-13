# ToolPlugin

## 类型

公开：
```text
ToolPlugin：工具路由插件，公开结构体--安装模板注册表及定义、调用路由System
ToolPluginInstalled：安装标记，公开Resource--供依赖插件确认ToolPlugin已安装

ToolTemplate：工具模板，公开结构体--一个可被模型调用的通用Loader或静态工具定义
    id: ResourceId--完整tool资源ID，必须使用type=tool
    definition: ToolDefinition--模型边界可见的工具定义
    id(&self) -> &ResourceId
    definition(&self) -> &ToolDefinition
    impl Clone for ToolTemplate

ToolDefinitionRequest：工具定义检查请求，公开事件--请求检查一个可见资源是否可用
    id: String--请求ID
    agent: Entity--发起检查的Agent
    resource: ResourceId--待检查的完整资源ID

ToolDefinitionRoute：工具定义路由，公开事件--发送给匹配的工具定义Plugin
    id: String
    agent: Entity
    loader: ResourceId--已注册的Loader模板ID
    resource: ResourceId--待检查的资源ID

ToolDefinitionResult：工具定义检查结果，公开事件
    id: String
    agent: Entity
    resource: ResourceId
    result: Result<ToolDefinition, ToolError>

ToolCallRequest：领域工具调用请求，公开事件--AgentPlugin交给ToolPlugin路由
    id: String--交互轮次ID
    agent: Entity
    call: ToolCall--调用ID、完整ResourceId和参数

ToolCallEvent：工具调用路由事件，公开事件--发送给具体工具定义Plugin
    id: String
    agent: Entity
    loader: ResourceId--匹配的Loader模板ID
    resource: ResourceId--被调用的完整资源ID
    call: ToolCall--保持原调用ID、ResourceId和参数

ToolError：工具路由错误，公开结构体--不包含资源正文、完整参数或绝对路径
    kind: ToolErrorKind
    message: String
    impl fmt::Display for ToolError
```

私有：
```text
ToolRegistry：工具模板注册表，私有Resource
    templates: BTreeMap<ResourceId, ToolTemplate>
    insert(&mut self, template: ToolTemplate) -> Result<(), ToolError>
    get(&self, id: &ResourceId) -> Option<&ToolTemplate>
```

## 函数

```text
register_tool_template(app: &mut App, template: ToolTemplate)
    注册模板：按完整tool ResourceId拒绝重复ID

tool_definition_route_system(world: &mut World)
    定义路由：读取ToolDefinitionRequest
    行为：根据resource.resource_type选择tool:builtin/<type>-loader:latest；模板不存在则发送失败ToolDefinitionResult；存在则发送ToolDefinitionRoute

tool_call_route_system(world: &mut World)
    调用路由：读取ToolCallRequest
    行为：type=tool时检查并直接转发该工具；type=skill时选择skill-loader，将Skill ResourceId注入Loader调用参数后发送ToolCallEvent；Loader不存在则为原tool_call_id发送失败AgentMessage::Tool

WorldToolExt::tool_definition_for(resource: &ResourceId) -> Option<ToolDefinition>
    定义查询：type=tool时返回注册表中的静态模板；type=skill时基于skill-loader返回name=Skill ResourceId且无参数的定义；所有返回定义的name均为完整ResourceId
```

## 逻辑

```text
工具定义Plugin安装时只注册通用Loader模板，不为每个Skill或Workflow注册条目。
ToolPlugin只检查Loader是否注册，不读取资源、不解析参数、不执行handler、不检查Agent可见性。
具体资源是否存在、定义是否合法、调用如何执行，全部由接收ToolCallEvent和ToolDefinitionRoute的Plugin负责。
```

## 边界

```text
AgentPlugin -> ToolCallRequest
ToolPlugin -> ToolCallEvent / ToolDefinitionRoute
SkillPlugin、WorkflowPlugin或其他工具Plugin -> AgentMessage::Tool / ToolDefinitionResult
ToolPlugin不依赖具体资源Plugin，也不保存每个资源的运行时实例。

共享Loader只注册一份内部模板，但每个可见Skill都生成一份以自身ResourceId命名、无参数的模型工具定义。
模型不传Skill ResourceId；ToolPlugin在路由Skill调用时由后端注入该ResourceId。
```
