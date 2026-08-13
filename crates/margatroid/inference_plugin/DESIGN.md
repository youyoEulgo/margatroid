# InferencePlugin

## 类型

公开：
```text
InferenceRequestEvent：推理请求事件，公开事件--AgentPlugin交付完整上下文和Provider无关ToolSpec
    id: String--交互轮次ID
    agent: Entity--请求所属Agent Entity
    agent_id: ResourceId--稳定Agent身份，用于流式消息和日志
    messages: Vec<Message>--System、长期对话和当前工具上下文
    tools: Vec<ToolDefinition>--内部ToolSpec；name已经是所属AgentToolMap分配的tool_name
    impl Event for InferenceRequestEvent

ProviderInferenceResponse：Provider响应，公开结构体--InferencePlugin内部的协议无关累积结果
    content: Option<String>--Assistant文本
    tool_calls: Vec<ToolCall>--保留Provider返回的调用ID、tool_name和参数
    stop_reason: StopReason
    usage: Option<TokenUsage>

InferenceError：推理错误，公开结构体
```

私有：
```text
PreparedInference：已准备推理，私有事件--主线程完成路由、Provider适配和发送器解析后的异步任务输入
    route: InferenceRoute
    agent_id: ResourceId
    client: reqwest::Client
    request: ProviderHttpRequest
    adapter: ErasedProviderAdapter
    senders: Vec<WebSocketSender>

InferenceTaskOutput：异步推理结果，私有事件
    route: InferenceRoute
    result: Result<ProviderInferenceResponse, InferenceError>

InferenceToolCall：Provider Adapter内部调用累积结构，私有结构体
    id: String
    tool_name: String
    arguments: String
```

## 函数

```text
prepare_inference_system(world: &mut World)
    准备推理：私有System，读取InferenceRequestEvent
    行为：
        验证请求、Agent和消息结构
        读取AgentInferenceSnapshot及Workspace或全局模型路由
        把内部ToolSpec交给选定Provider Adapter构造Provider请求
        不查询ToolPlugin，不读取AgentToolMap，不转换ResourceId
        根据全局配置解析流式发送器并发送PreparedInference

execute_prepared_inference(prepared: PreparedInference, context: AsyncContext)
    执行推理：私有异步函数
    行为：发送HTTP请求，按Provider协议累积文本和工具调用；文本分片直接按顺序发给前端，不进入事件队列

publish_inference_output_system(world: &mut World)
    发布结果：私有System，读取InferenceTaskOutput
    行为：
        失败时发送AgentFailure { kind: Inference }
        成功时使用ProviderInferenceResponse构造Message::Assistant { content, tool_calls }
        发送AgentMessage { id: route.id, agent: route.agent, message }
        不再发布InferenceResponse，也不经过工具身份转换System

ProviderAdapter::build_request(input: ProviderInput) -> Result<ProviderHttpRequest, InferenceError>
    构造Provider请求：把Provider无关ToolSpec转换成对应API的ToolSpec
    行为：ToolSpec.name原样使用AgentToolMap分配的tool_name；不编码ResourceId

ProviderAccumulator::finish(self) -> Result<ProviderInferenceResponse, InferenceError>
    完成响应：把Provider工具调用统一为ToolCall { id, tool_name, arguments }
    行为：OpenAI arguments字符串原样保留；对象型Provider参数序列化成JSON对象文本；无参数统一为"{}"
```

## 逻辑

```text
AgentPlugin
    -> InferenceRequestEvent { messages, tools }
InferencePlugin主线程
    -> Provider Adapter把内部ToolSpec转换为Provider ToolSpec
    -> PreparedInference
异步HTTP
    -> 直接发送有序文本分片
    -> 累积ProviderInferenceResponse
主线程发布
    -> AgentMessage::Assistant { content, tool_calls }
AgentPlugin
    -> tool_calls为空时结束轮次
    -> tool_calls非空时发送ToolCallEvent
```

## 边界

```text
InferencePlugin负责模型路由、Provider协议适配、HTTP请求、流式输出、响应累积和异步结果发布。
InferencePlugin不负责ResourceId与tool_name转换；tool_name由AgentToolMap在注册时确定并贯穿ToolSpec和ToolCall。
InferencePlugin不查询Agent可见性或工具注册状态，InferenceRequestEvent中的tools是本次请求唯一工具输入。
不存在公开InferenceResponse事件；异步结果发布System只负责把成功结果包装成AgentMessage。
模型参数不得用于选择Skill、Tool或其他资源。
```
