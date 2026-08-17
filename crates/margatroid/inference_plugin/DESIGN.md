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

ContextCompactionInferenceRequest：上下文压缩推理请求，公开事件--AgentPlugin交付System、待压缩头部消息和压缩提示词
    id: String--压缩请求ID
    agent: Entity--请求所属Agent Entity
    agent_id: ResourceId--稳定Agent身份，只用于路由和日志
    messages: Vec<Message>--不含保留尾部的压缩输入
    impl Event for ContextCompactionInferenceRequest

ContextCompactionInferenceResponse：上下文压缩推理响应，公开事件--不进入普通AgentMessage链路
    id: String--压缩请求ID
    agent: Entity--请求所属Agent Entity
    result: Result<String, InferenceError>--成功时为非空完整摘要正文，失败时为稳定推理错误
    impl Event for ContextCompactionInferenceResponse

CancelInferenceRequest：取消推理请求，公开事件--AgentPlugin中止轮次时发送
    id: String--要取消的交互轮次ID
    agent: Entity--请求所属Agent
    impl Event for CancelInferenceRequest

ProviderInferenceResponse：Provider响应，公开结构体--InferencePlugin内部的协议无关累积结果
    reasoning: Option<String>--Provider公开的完整思考内容
    content: Option<String>--Assistant文本
    tool_calls: Vec<ToolCall>--保留Provider返回的调用ID、tool_name和参数
    stop_reason: StopReason
    usage: Option<TokenUsage>--Provider返回usage时保存输入、输出和缓存命中Token；Provider完全不返回usage时为空

ProviderStreamDelta：Provider流式增量，公开枚举--Accumulator区分思考与正文的有序输出
    Reasoning(String)--新增思考文本
    Content(String)--新增正文文本

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
    senders: Vec<WebSocketSender>--普通Agent推理的流式目标；上下文压缩为空
    cancellation: watch::Receiver<bool>--当前请求取消信号

InFlightInferences：飞行中推理表，私有Resource
    requests: HashMap<(Entity, String), watch::Sender<bool>>--按Agent和turn_id定位取消信号

ModelRouteConfig：模型路由配置，私有结构体--由models.toml读取Provider协议及地址
    api_type: String--选择Provider Adapter；deepseek表示DeepSeek协议语义
    thinking: Option<String>--DeepSeek思考开关；enabled启用，省略或disabled关闭
    reasoning_effort: Option<String>--DeepSeek思考强度；启用思考时使用high或max

InferenceTaskOutput：异步推理结果，私有事件
    route: InferenceRoute
    result: InferenceTaskResult

InferenceTaskResult：异步推理结局，私有枚举
    Completed(Result<ProviderInferenceResponse, InferenceError>)
    Cancelled

InferenceToolCall：Provider Adapter内部调用累积结构，私有结构体
    id: String
    tool_name: String
    arguments: String
```

## 函数

```text
prepare_inference_system(world: &mut World)
    准备推理：私有System，分别读取InferenceRequestEvent和ContextCompactionInferenceRequest
    行为：
        验证请求、Agent和消息结构
        读取AgentInferenceSnapshot及Workspace或全局模型路由
        把内部ToolSpec交给选定Provider Adapter构造Provider请求
        不查询ToolPlugin，不读取AgentToolMap，不转换ResourceId
        普通Agent推理根据全局配置解析流式发送器；上下文压缩使用空ToolSpec和空发送器
        登记InFlightInferences并发送带结果用途的PreparedInference
        准备失败时，普通推理发送AgentFailure，上下文压缩发送ContextCompactionInferenceResponse::Err

cancel_inference_system(world: &mut World)
    取消推理：私有System，读取CancelInferenceRequest并向匹配飞行中请求发送取消信号

execute_prepared_inference(prepared: PreparedInference, context: AsyncContext)
    执行推理：私有异步函数
    行为：发送HTTP请求，按Provider协议累积思考、文本和工具调用；思考与文本分片分别直接按顺序发给前端，不进入事件队列；取消信号到达时丢弃HTTP Future并返回Cancelled

publish_inference_output_system(world: &mut World)
    发布结果：私有System，读取InferenceTaskOutput
    行为：
        从InFlightInferences移除当前请求；已取消结果不发送AgentMessage或AgentFailure
        普通推理失败时发送AgentFailure { kind: Inference }
        普通推理成功时使用ProviderInferenceResponse构造Message::Assistant并发送携带response.usage的AgentMessage
        上下文压缩失败时发送ContextCompactionInferenceResponse::Err
        上下文压缩成功时要求stop_reason=Completed、无tool_calls且content为非空正文；只发送ContextCompactionInferenceResponse::Ok(content)
        不再发布InferenceResponse，也不经过工具身份转换System

ProviderAdapter::build_request(input: ProviderInput) -> Result<ProviderHttpRequest, InferenceError>
    构造Provider请求：把Provider无关ToolSpec转换成对应API的ToolSpec
    行为：ToolSpec.name原样使用AgentToolMap分配的tool_name；不编码ResourceId；OpenAI兼容流设置stream_options.include_usage=true以请求末尾Token统计

ProviderAccumulator::finish(self) -> Result<ProviderInferenceResponse, InferenceError>
    完成响应：把Provider工具调用统一为ToolCall { id, tool_name, arguments }
    行为：OpenAI arguments字符串原样保留；对象型Provider参数序列化成JSON对象文本；无参数统一为"{}"

DeepSeekAdapter::build_request(input: ProviderInput) -> Result<ProviderHttpRequest, InferenceError>
    构造DeepSeek请求：使用/chat/completions并发送thinking与reasoning_effort
    行为：Assistant有tool_calls且reasoning非空时写reasoning_content；普通Assistant历史不回传reasoning

DeepSeekAccumulator::push(chunk: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError>
    累积DeepSeek流：分别解析reasoning_content和content
    行为：原生reasoning_content或OpenRouter兼容reasoning产生Reasoning分片，content产生Content分片，二者都按Provider顺序返回

decode_token_usage(usage: OpenAiUsage) -> TokenUsage
    统一Token统计：私有函数
    行为：输入与输出兼容input_tokens/output_tokens和prompt_tokens/completion_tokens；缓存命中依次兼容prompt_tokens_details.cached_tokens、input_tokens_details.cached_tokens和prompt_cache_hit_tokens；有usage但无缓存字段时cache_hit_tokens为0
```

## 逻辑

```text
AgentPlugin
    -> InferenceRequestEvent { messages, tools }
InferencePlugin主线程
    -> Provider Adapter把内部ToolSpec转换为Provider ToolSpec
    -> PreparedInference
异步HTTP
    -> reasoning_content直接发送agent.message.reasoning_delta
    -> content直接发送agent.message.delta
    -> 累积ProviderInferenceResponse
主线程发布
    -> AgentMessage { Message::Assistant { reasoning, content, tool_calls }, usage }
AgentPlugin
    -> tool_calls为空时结束轮次
    -> tool_calls非空时发送ToolCallEvent

AgentPlugin
    -> ContextCompactionInferenceRequest { messages, tools=[] }
InferencePlugin异步HTTP
    -> 不发送流式前端消息
InferencePlugin主线程发布
    -> ContextCompactionInferenceResponse
AgentPlugin
    -> 校验上下文快照并rewrite_messages
```

## 边界

```text
InferencePlugin负责模型路由、Provider协议适配、HTTP请求、流式输出、响应累积和异步结果发布。
InferencePlugin不负责ResourceId与tool_name转换；tool_name由AgentToolMap在注册时确定并贯穿ToolSpec和ToolCall。
InferencePlugin不查询Agent可见性或工具注册状态，InferenceRequestEvent中的tools是本次请求唯一工具输入。
上下文压缩请求始终使用空ToolSpec、空流式发送器和独立响应事件；摘要不会成为普通Assistant消息。
上下文压缩Provider响应即使带usage也不进入AgentMessage或历史Token统计。
不存在公开InferenceResponse事件；异步结果发布System只负责把成功结果包装成AgentMessage。
模型参数不得用于选择Skill、Tool或其他资源。
OpenAI Adapter不回传或解析Message::Assistant.reasoning；DeepSeek Adapter独占DeepSeek协议差异。
api_type决定协议语义，base_url只决定请求目的地；经过OpenAI兼容反代调用DeepSeek时仍使用api_type=deepseek。
```
