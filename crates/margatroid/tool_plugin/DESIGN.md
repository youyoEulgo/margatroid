# ToolPlugin

## 类型

公开：
```text
ToolPlugin：Agent专属资源映射与调用管理插件，公开结构体--提供AgentResourceMap注册接口及工具调用System
    impl Plugin for ToolPlugin
        build(self, app: &mut App)
            构建插件：公开插件入口
            行为：
                重复安装时panic
                插入ToolPluginInstalled；每个Agent的飞行中调用只保存在Agent.tools.pending
                注册AgentResourceRegisterRequest、AgentResourceRegisterResponse、ToolCallEvent、ToolCallRequest、ToolCallResponse和CancelToolTurn事件
                挂载tool_call_route_system、tool_call_response_system和cancel_tool_turn_system
ToolPluginInstalled：安装标记，公开Resource--供依赖Plugin确认ToolPlugin已安装

ToolTemplate：内部工具规格，公开结构体--Provider无关的模型工具定义
    name: String--ResourceMapEntry.resource_name；InferencePlugin再转换为Provider合法名称
    description: String--模型可见说明
    parameters: serde_json::Value--Provider无关的JSON Schema参数
    impl Clone for ToolTemplate

ResourceContent：非工具资源的已解析内容，公开枚举
    Prompt { role: MessageRole, content: Arc<str> }
    impl Clone for ResourceContent

ResourceMapEntry：Agent资源映射，公开结构体--同时表示可执行资源和普通内容资源
    resource_id: ResourceId--本映射代表的完整资源ID
    resource_name: String--当前Agent内有效名称；有alias时使用alias，否则使用完整ResourceId字符串
    alias: Option<String>--IMPORT显式别名
    tool_id: Option<ResourceId>--Some表示由该隐藏内建工具执行，None表示不可调用资源
    template: Option<ToolTemplate>--模型工具规格；与tool_id同时为Some或同时为None
    content: Option<ResourceContent>--Prompt等普通资源内容；可执行资源通常为None
    impl Clone for ResourceMapEntry

AgentResourceMap：Agent专属资源表，公开结构体--只存放在Agent.resources，不作为第二个Component挂载
    resources: Vec<ResourceMapEntry>--当前Agent已经解析的全部资源映射，私有
    get_by_name(&self, resource_name: &str) -> Option<&ResourceMapEntry>
        按名称查询：公开方法，resource_name和alias在当前Agent内唯一
    get_by_tool(&self, tool_id: &ResourceId) -> Vec<&ResourceMapEntry>
        按工具查询：公开方法，返回由同一内部工具处理的全部映射
    get_by_resource(&self, resource_id: &ResourceId) -> Vec<&ResourceMapEntry>
        按资源查询：公开方法，返回对应具体资源的全部映射
    register(&mut self, entry: ResourceMapEntry) -> Result<&ResourceMapEntry, ToolError>
        注册映射：公开方法；相同resource_id和相同alias已存在时返回原映射，作为幂等成功
        其他重复resource_name或重复alias返回DuplicateResource，且不修改原映射
        同一resource_id可以由不同Driver使用不同alias注册为不同映射
        template存在时强制template.name等于resource_name
        IMPORT的alias只随完整ResourceMapEntry一次性注册；Provider验证完成前不单独写入alias
        任何领域消息、历史记录、实时上下文和ToolSpec都优先使用entry.alias；资源无alias时才使用完整ResourceId字符串

AgentResourceRegisterRequest：Agent资源注册请求，公开事件--注册协议类型由ToolPlugin提供，具体Provider负责消费
    id: String--发起注册的Plugin生成的内部唯一请求ID；MCL IMPORT使用"mcl-import:"加MclCommandId字符串值
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--待建立ResourceMapEntry的资源ID，不允许模型直接导入tool:builtin/*
    alias: Option<String>--MCL IMPORT声明的可选别名
    impl Event for AgentResourceRegisterRequest
    impl Clone for AgentResourceRegisterRequest

AgentResourceRegisterResponse：Agent资源注册响应，公开事件--具体Provider无论成功失败都恰好响应一次
    id: String--原注册请求ID
    agent: Entity--原目标Agent Entity
    resource_id: ResourceId--原资源ID
    alias: Option<String>--原请求alias
    result: Result<ResourceMapEntry, ToolError>--成功返回尚未写入AgentResourceMap且已通过真实存在性与可用性测试的候选映射
    impl Event for AgentResourceRegisterResponse
    impl Clone for AgentResourceRegisterResponse

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

CancelToolTurn：取消工具批次，公开事件--Agent生命周期控制或MclPlugin清理已结束VM时发送
    turn_id: String
    agent: Entity
    impl Event for CancelToolTurn

ToolError：工具错误，公开结构体--不包含资源正文、完整参数或绝对路径
    kind: ToolErrorKind
    message: String
    impl fmt::Display for ToolError
    impl Clone for ToolError

ToolErrorKind：工具错误分类，公开枚举
    AgentMissing
    ResourceMapMissing
    InvalidResource
    ResourceUnavailable
    DuplicateResource
    RegistrationFailed
    ToolCallMissing
    ExecutionFailed
    impl Clone for ToolErrorKind
```

## 函数

```text
register_agent_resource(world: &mut World, agent: Entity, entry: ResourceMapEntry) -> Result<ResourceMapEntry, ToolError>
    注册Agent资源：公开函数，取得唯一Agent组件中的Agent.resources并调用register；由MclPlugin在IMPORT提交事务中调用，具体Provider不得提前调用

resolve_agent_tool_definitions(world: &World, agent: Entity, resources: &[ResourceId]) -> Result<Vec<ToolDefinition>, ToolError>
    解析本次可见工具：公开函数，按resources顺序从Agent.resources找到已导入且template存在的唯一映射，克隆成ToolDefinition；任一资源缺失、重复或不可执行时整体失败

validate_agent_tool_calls(world: &World, agent: Entity, calls: &[ToolCall]) -> Result<(), ToolError>
    预检工具调用：公开函数，在MclPlugin发送任何ToolCallEvent前验证每个tool_name映射到Agent.resources中的唯一可执行项；不登记pending、不执行工具

tool_call_route_system(world: &mut World)
    路由调用：私有System，读取ToolCallEvent
    行为：
        验证turn_id、Agent和ToolCall ID
        从Agent Entity的唯一Agent组件取得Agent.resources并按call.tool_name查询唯一映射
        映射的tool_id和template必须为Some，否则拒绝调用
        构造ToolCallRequest { turn_id, agent, tool_id, resource_id, tool_call_id, arguments }
        在发送请求前加入Agent.tools.pending
        映射缺失或请求重复时发送AgentFailure { id: turn_id, kind: Tool }，由MclPlugin关闭当前或下一次start；不伪造Tool成功消息

tool_call_response_system(world: &mut World)
    整理响应：私有System，读取ToolCallResponse
    行为：
        使用Agent、turn_id和tool_call_id从Agent.tools.pending移除原请求
        未找到原请求时记录稳定错误，不发布AgentMessage
        使用原请求.resource_id、响应tool_call_id及成功正文或稳定错误文本构造Message::Tool
        发送AgentMessage { id: turn_id, agent, message, usage: None }

cancel_tool_turn_system(world: &mut World)
    取消工具批次：私有System，从Agent.tools.pending移除指定Agent与turn_id的全部请求；迟到响应因无法匹配而丢弃
```

## 逻辑

```text
注册：
    AgentPlugin创建Agent
        -> 在唯一Agent组件中初始化空AgentResourceMap
    Base Driver执行IMPORT
        -> MclPlugin发送AgentResourceRegisterRequest
        -> BuiltinToolPlugin按资源类型路由到具体Provider
        -> 具体工具Plugin验证资源并返回候选ResourceMapEntry
        -> AgentResourceRegisterResponse
        -> MclPlugin在IMPORT事务中调用register_agent_resource并提交导入

调用：
    Base Lua的tool_call Effect -> MclPlugin预检全部调用 -> ToolCallEvent { turn_id, agent, call }
    ToolPlugin -> AgentResourceMap.get_by_name(call.tool_name)
               -> Agent.tools.pending登记请求
               -> ToolCallRequest
    具体工具Plugin -> ToolCallResponse
    ToolPlugin -> Agent.tools.pending移除请求
               -> AgentMessage { Message::Tool { resource_id, tool_call_id, content } }
    AgentPlugin -> 长期Lua VM邮箱
    Base Lua的start Effect -> 取得Tool消息，追加消息并删除对应pending_tool
                             -> pending_tool为空时发起下一次inference Effect
```

## 边界

```text
AgentResourceMap只存在于Agent.resources，resource_name和alias只在该Agent内唯一；不建立第二个Component或全局名称索引。
ToolPlugin定义AgentResourceMap行为并独占读写Agent.tools.pending，负责映射、路由、调用关联和响应整理；不判断MCL语义上的批次完成。
ToolPlugin定义AgentResourceRegisterRequest和AgentResourceRegisterResponse作为中立注册协议，但不消费注册请求、不选择资源Provider。
Margatroid应用组合必须安装一个注册路由消费者；当前由BuiltinToolPlugin消费全部请求并保证每个请求恰好一个响应。
ToolPlugin不读取Skill或Workflow文件，不解析具体参数，不执行工具，不检查Agent可见性。
AgentResourceMap注册成功后长期保留；从TOOL数组移除可见性不删除映射。
同一resource_id和alias重新导入时复用既有ResourceMapEntry；ToolPlugin仍不决定资源何时进入TOOL数组。
具体工具Plugin负责资源清单允许后的资源验证、真实存在性与可用性测试、候选ResourceMapEntry构造和执行，不写AgentResourceMap，不自行构造AgentMessage。
工具执行关联统一保存在Agent.tools.pending；get_by_turn由ToolPlugin读取该字段，不再挂载独立PendingToolCalls组件或保存第二份Agent工具状态。
具体工具Plugin必须对每个ToolCallRequest恰好发送一个ToolCallResponse；执行失败放入response.result，ToolPlugin仍把它转换成可配对的Tool消息，使Base Lua能够删除pending_tool并继续或结束循环。
```
