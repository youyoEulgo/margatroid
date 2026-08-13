# InferencePlugin

## 类型
公开：
```text
InferenceRequest：推理请求，公开事件
    id: String
    agent: Entity
    agent_id: ResourceId
    messages: Vec<Message>
    tools: Vec<ToolDefinition>--name为完整ResourceId
    impl Event for InferenceRequest

InferenceResponse：推理响应，公开事件--Provider完成后发送，随后由转换System发布AgentMessage
    id: String
    agent: Entity
    response: ProviderInferenceResponse
    tool_resources: BTreeMap<String, ResourceId>--API工具名到领域ResourceId的一对一映射
    impl Event for InferenceResponse

ProviderInferenceResponse：Provider响应，公开结构体--InferencePlugin内部的协议无关累积结果
    message: Message--当前仍保存模型工具名，不能离开InferencePlugin
    stop_reason: StopReason
    usage: Option<TokenUsage>

InferenceError：推理错误，公开结构体
```

## 函数
```text
prepare_inference_system(world: &mut World)
    准备请求：读取InferenceRequest，按AgentImage和Workspace路由表构造Provider请求
    行为：将每个ToolDefinition.name中的ResourceId转换成API合法工具名，保存API名到ResourceId映射，再构造Provider请求

api_tool_name(resource: &ResourceId) -> String
    模型名称转换：将完整ResourceId稳定转换成API工具名，例如skill:local/review:latest -> skill_local_review_latest

convert_resource_tools(tools: &[ToolDefinition]) -> Result<(Vec<ToolDefinition>, BTreeMap<String, ResourceId>), InferenceError>
    请求工具转换：解析每个定义名中的ResourceId，替换为API工具名，并拒绝转换后重名

execute_prepared_inference(command: PreparedInference, context: AsyncContext)
    执行推理：发送HTTP请求，累积ProviderInferenceResponse并发布异步结果

publish_inference_output_system(world: &mut World)
    发布响应：将Provider结果包装为InferenceResponse；请求失败发布AgentFailure

convert_inference_response_system(world: &mut World)
    转换响应：读取InferenceResponse
    行为：按响应携带的API名到ResourceId映射转换每个工具调用；未知API名发布AgentFailure；参数原样保留

convert_inference_tool_call(call: InferenceToolCall, tool_resources: &BTreeMap<String, ResourceId>) -> Result<ToolCall, String>
    响应工具转换：只按InferencePlugin保存的一对一映射还原ResourceId，不读取参数中的资源字段
```

## 边界
```text
InferencePlugin负责模型协议、Provider路由、HTTP请求和模型工具名到资源ID的转换。
API工具名只存在于InferencePlugin内部；InferenceRequest和AgentMessage中的工具名均使用ResourceId。
模型参数不得用于选择Skill、Tool或其他资源。
InferenceResponse不得直接绕过转换System成为AgentMessage。
AgentPlugin只接收转换后的AgentMessage，并将资源ID调度为ToolCallRequest。
```
