# ToolPlugin

## 类型

公开：
```text
ToolPlugin：Agent专属工具映射与调用管理插件，公开结构体--安装AgentToolMap注册接口、PendingToolCalls及工具调用System
ToolPluginInstalled：安装标记，公开Resource--供依赖Plugin确认ToolPlugin已安装

ToolTemplate：内部工具规格，公开结构体--Provider无关的模型工具定义
    name: String--Agent内唯一的模型工具名，由AgentToolMap注册时写入
    description: String--模型可见说明
    parameters: serde_json::Value--Provider无关的JSON Schema参数

ToolMap：Agent工具映射，公开结构体--关联模型名称、内部执行工具和具体资源
    tool_name: String--模型看到并返回的名称，只在所属Agent内唯一
    tool_id: ResourceId--实际处理调用的内部工具ID
    resource_id: ResourceId--本映射代表的具体资源ID
    template: ToolTemplate--内部工具规格，name与tool_name一致

AgentToolMap：Agent专属工具表，公开Component--由ToolPlugin挂载到Agent Entity
    next_index: u64--当前Agent下一代数，私有；删除映射时不回收
    tools: Vec<ToolMap>--当前Agent的全部工具映射，私有
    get_by_name(&self, tool_name: &str) -> Option<&ToolMap>
        按名称查询：公开方法，在当前Agent内按唯一tool_name定位映射
    get_by_tool(&self, tool_id: &ResourceId) -> Vec<&ToolMap>
        按工具查询：公开方法，返回由同一内部工具处理的全部映射
    get_by_resource(&self, resource_id: &ResourceId) -> Vec<&ToolMap>
        按资源查询：公开方法，返回对应具体资源的全部映射
    register(&mut self, tool_id: ResourceId, resource_id: ResourceId, template: ToolTemplate) -> Result<&ToolMap, ToolError>
        注册映射：公开方法，使用当前next_index生成Agent内唯一tool_name，写入template.name后递增代数
        命名：格式为<resource type><index>_<resource name>；资源名中非ASCII字母、数字、下划线或短横线的字符替换为下划线，最终超过64字符直接截断
        行为：同一Agent内拒绝重复resource_id映射；不执行工具、不保存测试参数
    impl Component for AgentToolMap

AgentToolRegisterRequest：Agent资源工具注册请求，公开事件--注册协议类型由ToolPlugin提供，具体Provider负责消费
    id: String--AgentPlugin生成的内部唯一注册请求ID
    agent: Entity--目标Agent Entity
    resource_id: ResourceId--待建立ToolMap的模型可见资源ID，不允许tool:builtin/*
    impl Event for AgentToolRegisterRequest

AgentToolRegisterResponse：Agent资源工具注册响应，公开事件--具体Provider无论成功失败都恰好响应一次
    id: String--原注册请求ID
    agent: Entity--原目标Agent Entity
    resource_id: ResourceId--原资源ID
    result: Result<(), ToolError>--成功只表示AgentToolMap已经建立，不表示资源已进入动态可见性
    impl Event for AgentToolRegisterResponse

ToolCallEvent：模型或后端发起的工具调用事件，公开事件--交给ToolPlugin解析Agent专属映射
    turn_id: String--完整交互轮次ID
    agent: Entity--调用所属Agent Entity
    call: ToolCall--调用ID、Agent内tool_name和参数
    impl Event for ToolCallEvent

ToolCallRequest：内部工具执行请求，公开事件--ToolPlugin解析ToolMap后交给具体工具Plugin
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

ToolTurnCompleted：工具批次完成事件，公开事件--通知AgentPlugin同轮全部工具均已响应
    turn_id: String
    agent: Entity
    impl Event for ToolTurnCompleted

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
    calls: Vec<ToolCallRequest>
    get(&self, agent: Entity, turn_id: &str, tool_call_id: &str) -> Option<&ToolCallRequest>
        精确查询：私有方法，使用Agent、turn和tool call ID完整定位
    get_by_agent(&self, agent: Entity) -> Vec<&ToolCallRequest>
        按Agent查询：私有方法
    get_by_turn(&self, agent: Entity, turn_id: &str) -> Vec<&ToolCallRequest>
        按轮次查询：私有方法，供批次完成判断
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
attach_agent_tool_map(world: &mut World, agent: Entity) -> Result<(), ToolError>
    挂载工具表：公开函数，由AgentPlugin创建Agent时调用；拒绝死亡Entity和重复挂载

register_agent_tool(world: &mut World, agent: Entity, tool_id: ResourceId, resource_id: ResourceId, template: ToolTemplate) -> Result<ToolMap, ToolError>
    注册Agent工具：公开函数，取得AgentToolMap并调用register；不检查Agent可见性、不读取资源、不执行工具

tool_call_route_system(world: &mut World)
    路由调用：私有System，读取ToolCallEvent
    行为：
        验证turn_id、Agent和ToolCall ID
        从Agent Entity取得AgentToolMap并按call.tool_name查询唯一映射
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
        移除后调用get_by_turn；为空时发送ToolTurnCompleted，否则继续等待
```

## 逻辑

```text
注册：
    AgentPlugin创建Agent
        -> 调用ToolPlugin挂载空AgentToolMap
    WorkspacePlugin挂载Agent运行组件并通知AgentPlugin恢复默认可见性
        -> AgentPlugin发送AgentToolRegisterRequest
        -> BuiltinToolPlugin按资源类型路由到具体Provider
        -> 具体工具Plugin验证可见资源
        -> register_agent_tool
        -> AgentToolMap为当前Agent分配tool_name
        -> AgentToolRegisterResponse
        -> AgentPlugin决定是否注入AgentDynamicVisibility

调用：
    AgentPlugin -> ToolCallEvent { turn_id, agent, call }
    ToolPlugin -> AgentToolMap.get_by_name(call.tool_name)
               -> PendingToolCalls.add_pending
               -> ToolCallRequest
    具体工具Plugin -> ToolCallResponse
    ToolPlugin -> PendingToolCalls.remove
               -> AgentMessage { Message::Tool { resource_id, tool_call_id, content } }
               -> 本轮Pending为空时发送ToolTurnCompleted
```

## 边界

```text
AgentToolMap是Agent Entity上的Component，tool_name只在该Component内唯一；不建立全局tool_name索引。
ToolPlugin拥有AgentToolMap和PendingToolCalls，负责映射、路由、调用关联、响应整理和批次完成判断。
ToolPlugin定义AgentToolRegisterRequest和AgentToolRegisterResponse作为中立注册协议，但不消费注册请求、不选择资源Provider。
Margatroid应用组合必须安装一个注册路由消费者；当前由BuiltinToolPlugin消费全部请求并保证每个请求恰好一个响应。
ToolPlugin不读取Skill或Workflow文件，不解析具体参数，不执行工具，不检查Agent可见性。
AgentToolMap注册成功后长期保留；动态移除可见性不删除映射，也不回收next_index或tool_name。
同一资源重新可见时复用既有ToolMap；ToolPlugin仍不决定该资源何时可见。
具体工具Plugin负责资源验证、ToolTemplate构造和执行，只返回ToolCallResponse，不自行构造AgentMessage。
AgentStatus不保存pending tool；get_by_turn只属于PendingToolCalls。
```
