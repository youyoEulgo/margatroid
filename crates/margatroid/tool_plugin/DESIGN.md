# ToolPlugin

## 类型

公开：
```text
ToolPlugin：Agent专属资源映射与调用管理插件，公开结构体--安装AgentResourceMap注册接口、PendingToolCalls及工具调用System
ToolPluginInstalled：安装标记，公开Resource--供依赖Plugin确认ToolPlugin已安装

ToolTemplate：内部工具规格，公开结构体--Provider无关的模型工具定义
    name: String--ResourceMapEntry.resource_name；InferencePlugin再转换为Provider合法名称
    description: String--模型可见说明
    parameters: serde_json::Value--Provider无关的JSON Schema参数

ResourceContent：非工具资源的已解析内容，公开枚举
    Prompt { role: MessageRole, content: Arc<str> }

ResourceMapEntry：Agent资源映射，公开结构体--同时表示可执行资源和普通内容资源
    resource_id: ResourceId--本映射代表的完整资源ID
    resource_name: String--当前Agent内有效名称；有alias时使用alias，否则使用完整ResourceId字符串
    alias: Option<String>--IMPORT显式别名
    tool_id: Option<ResourceId>--Some表示由该隐藏内建工具执行，None表示不可调用资源
    template: Option<ToolTemplate>--模型工具规格；与tool_id同时为Some或同时为None
    content: Option<ResourceContent>--Prompt等普通资源内容；可执行资源通常为None

AgentResourceMap：Agent专属资源表，公开Component--由ToolPlugin挂载到Agent Entity
    resources: Vec<ResourceMapEntry>--当前Agent已经解析的全部资源映射，私有
    get_by_name(&self, resource_name: &str) -> Option<&ResourceMapEntry>
        按名称查询：公开方法，resource_name和alias在当前Agent内唯一
    get_by_tool(&self, tool_id: &ResourceId) -> Vec<&ResourceMapEntry>
        按工具查询：公开方法，返回由同一内部工具处理的全部映射
    get_by_resource(&self, resource_id: &ResourceId) -> Vec<&ResourceMapEntry>
        按资源查询：公开方法，返回对应具体资源的全部映射
    register(&mut self, entry: ResourceMapEntry) -> Result<&ResourceMapEntry, ToolError>
        注册映射：公开方法，拒绝重复resource_name和重复alias
        同一resource_id可以由不同Driver使用不同alias注册为不同映射
        template存在时强制template.name等于resource_name
    set_alias(resource_id, alias)
        IMPORT声明别名时立即记录；资源已注册则同步替换resource_name和template.name
        任何领域消息、历史记录、实时上下文和ToolSpec都优先使用alias；代数名只允许作为无alias资源的回退
    impl Component for AgentResourceMap

AgentResourceRegisterRequest：Agent资源注册请求，公开事件--注册协议类型由ToolPlugin提供，具体Provider负责消费
    id: String--AgentPlugin生成的内部唯一注册请求ID
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--待建立ResourceMapEntry的资源ID，不允许模型直接导入tool:builtin/*
    alias: Option<String>--MCL IMPORT声明的可选别名
    impl Event for AgentResourceRegisterRequest

AgentResourceRegisterResponse：Agent资源注册响应，公开事件--具体Provider无论成功失败都恰好响应一次
    id: String--原注册请求ID
    agent: Entity--原目标Agent Entity
    resource_id: ResourceId--原资源ID
    alias: Option<String>--原请求alias
    result: Result<ResourceMapEntry, ToolError>--成功返回尚未写入AgentResourceMap的候选映射
    impl Event for AgentResourceRegisterResponse

ToolCallEvent：模型或后端发起的工具调用事件，公开事件--交给ToolPlugin解析Agent专属映射
    turn_id: String--完整交互轮次ID
    agent: Entity--调用所属Agent Entity
    call: ToolCall--调用ID、Agent内resource_name和参数
    impl Event for ToolCallEvent

ToolCallRequest：内部工具执行请求，公开事件--ToolPlugin解析ResourceMapEntry后交给具体工具Plugin
    turn_id: String
    agent: Entity
    tool_id: ResourceId--实际处理请求的内部工具
    resource_id: ResourceId--本次调用对应的具体资源
    tool_call_id: String--对应ToolCall.id
    arguments: String--合法JSON对象文本
    impl Event for ToolCallRequest

ToolCallResponse：工具执行结果，公开事件--只携带结果和定位原请求所需的信息
    turn_id: String
    agent: Entity
    tool_call_id: String
    result: Result<String, ToolError>--成功正文或稳定工具错误
    impl Event for ToolCallResponse

CancelToolTurn：取消工具批次，公开事件--AgentPlugin中止轮次时发送
    turn_id: String
    agent: Entity
    impl Event for CancelToolTurn

ToolError：工具错误，公开结构体--不包含资源正文、完整参数或绝对路径
    kind: ToolErrorKind
    message: String
    impl fmt::Display for ToolError
```

私有：
```text
PendingToolCalls：待执行工具调用池，私有Resource--由ToolPlugin独占
    只负责异步请求与响应关联，不是MCL语义中工具批次完成状态的事实来源
    calls: Vec<ToolCallRequest>
    get(&self, agent: Entity, turn_id: &str, tool_call_id: &str) -> Option<&ToolCallRequest>
        精确查询：私有方法，使用Agent、turn和tool call ID完整定位
    get_by_agent(&self, agent: Entity) -> Vec<&ToolCallRequest>
        按Agent查询：私有方法
    get_by_turn(&self, agent: Entity, turn_id: &str) -> Vec<&ToolCallRequest>
        按轮次查询：私有方法，供取消、诊断和清理使用
    add_pending(&mut self, request: ToolCallRequest) -> Result<(), ToolError>
        登记请求：私有方法，拒绝重复完整定位键
    remove(&mut self, agent: Entity, turn_id: &str, tool_call_id: &str) -> Option<ToolCallRequest>
        移除请求：私有方法，返回原请求供响应整理
    remove_turn(&mut self, agent: Entity, turn_id: &str)
        移除轮次：私有方法，删除指定Agent与turn_id的全部请求
    impl Resource for PendingToolCalls
```

## 函数

```text
attach_agent_resource_map(world: &mut World, agent: Entity) -> Result<(), ToolError>
    挂载资源表：公开函数，由AgentPlugin创建Agent时调用；拒绝死亡Entity和重复挂载

register_agent_resource(world: &mut World, agent: Entity, entry: ResourceMapEntry) -> Result<ResourceMapEntry, ToolError>
    注册Agent资源：公开函数，取得AgentResourceMap并调用register；由MclPlugin在IMPORT提交事务中调用，具体Provider不得提前调用

tool_call_route_system(world: &mut World)
    路由调用：私有System，读取ToolCallEvent
    行为：
        验证turn_id、Agent和ToolCall ID
        从Agent Entity取得AgentResourceMap并按call.tool_name查询唯一映射
        映射的tool_id和template必须为Some，否则拒绝调用
        构造ToolCallRequest { turn_id, agent, tool_id, resource_id, tool_call_id, arguments }
        在发送请求前加入PendingToolCalls
        映射缺失或请求重复时发送AgentFailure，不伪造Tool成功消息

tool_call_response_system(world: &mut World)
    整理响应：私有System，读取ToolCallResponse
    行为：
        使用Agent、turn_id和tool_call_id从PendingToolCalls移除原请求

cancel_tool_turn_system(world: &mut World)
    取消工具批次：私有System，移除指定Agent与turn_id的全部PendingToolCalls；迟到响应因无法匹配而丢弃
        未找到原请求时记录稳定错误，不发布AgentMessage
        使用原请求.resource_id、响应tool_call_id及结果正文构造Message::Tool
        发送AgentMessage { id: turn_id, agent, message }
```

## 逻辑

```text
注册：
    AgentPlugin创建Agent
        -> 调用ToolPlugin挂载空AgentResourceMap
    Base Driver执行IMPORT
        -> MclPlugin发送AgentResourceRegisterRequest
        -> BuiltinToolPlugin按资源类型路由到具体Provider
        -> 具体工具Plugin验证资源并返回候选ResourceMapEntry
        -> AgentResourceRegisterResponse
        -> MclPlugin在IMPORT事务中调用register_agent_resource并提交导入

调用：
    AgentPlugin -> ToolCallEvent { turn_id, agent, call }
    ToolPlugin -> AgentResourceMap.get_by_name(call.tool_name)
               -> PendingToolCalls.add_pending
               -> ToolCallRequest
    具体工具Plugin -> ToolCallResponse
    ToolPlugin -> PendingToolCalls.remove
               -> AgentMessage { Message::Tool { resource_id, tool_call_id, content } }
    AgentPlugin -> MCL追加Tool消息并删除对应pending_tool
                -> pending_tool为空时发起下一次推理
```

## 边界

```text
AgentResourceMap是Agent Entity上的Component，resource_name和alias只在该Component内唯一；不建立全局名称索引。
ToolPlugin拥有AgentResourceMap和PendingToolCalls，负责映射、路由、调用关联和响应整理；不判断MCL语义上的批次完成。
ToolPlugin定义AgentResourceRegisterRequest和AgentResourceRegisterResponse作为中立注册协议，但不消费注册请求、不选择资源Provider。
Margatroid应用组合必须安装一个注册路由消费者；当前由BuiltinToolPlugin消费全部请求并保证每个请求恰好一个响应。
ToolPlugin不读取Skill或Workflow文件，不解析具体参数，不执行工具，不检查Agent可见性。
AgentResourceMap注册成功后长期保留；从TOOL数组移除可见性不删除映射。
同一resource_id和alias重新导入时复用既有ResourceMapEntry；ToolPlugin仍不决定资源何时进入TOOL数组。
具体工具Plugin负责资源验证、候选ResourceMapEntry构造和执行，不写AgentResourceMap，不自行构造AgentMessage。
AgentStatus不保存pending tool；get_by_turn只属于PendingToolCalls。
```
