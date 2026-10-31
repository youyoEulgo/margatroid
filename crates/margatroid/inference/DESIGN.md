# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名和直接行为。
System分别列出事件、闭包结果、提交闭包及oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib       inference/core/src/lib.rs；inference/plugin/src/lib.rs  模型配置Resource；Plugin安装
# components inference/core/src/components.rs                    ActiveInference
# events    inference/core/src/events.rs                          InitInference、InferenceRequest
# system    inference/plugin/src/system.rs                        请求路由
# handler   inference/plugin/src/handler.rs                       受理、协议适配及流监督
# types     inference/plugin/src/types.rs                         路由适配器、接收器及累积器
# types pub inference/core/src/types.rs                           请求、消息、结果及模型配置
# error     inference/core/src/error.rs                           InferenceError
```

# lib

## 类型

公开，定义于inference_core：
```text
ModelRoutes：模型配置，公开Resource
    entries: BTreeMap<String, ModelRoute>--私有
    get(&self, id: &str) -> Option<&ModelRoute>
        行为：借用模型路由，不输出凭据。
    impl Resource for ModelRoutes
```

公开，定义于inference_plugin：
```text
InferencePlugin：单次推理执行插件
    schedule: String
    routes: ModelRoutes
    open(path: &Path) -> Result<Self, InferenceError>
        行为：有界读取models.toml，完整校验provider、api_type及路由选项；失败不构造插件。
    impl Plugin for InferencePlugin
        build(self, app: &mut App)
            行为：检查运行设施，拒绝重复安装；安装ModelRoutes及共享HTTP客户端，挂载init_inference_system和inference_system。
```

# components

## 类型

公开：
```text
ActiveInference：该Agent的当前推理句柄，公开组件
    current: Option<InferenceHandle>--初始化为None，不放入ToolTasks
    impl Component for ActiveInference
```

# events

## 类型

公开：
```text
InitInference：推理句柄组件初始化请求，公开事件
    agent: Entity
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), InferenceError>>>>>
    impl Event for InitInference

InferenceRequest：推理请求，公开事件
    id: String
    agent: Entity
    turn: String
    messages: Vec<InferenceMessage>
    tools: Vec<String>--已导入工具alias，空数组表示无工具
    config: InferenceParameters
    cancel: CancellationToken
    reply: Arc<Mutex<Option<oneshot::Sender<Result<InferenceResponse, InferenceError>>>>>
    impl Event for InferenceRequest
```

# system

## 函数

crate公开：
```text
init_inference_system(world: &mut World)
    处理事件：InitInference
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    oneshot回复：Result<(), InferenceError>
    行为：InitInference -> handle_init_inference。

inference_system(world: &mut World)
    处理事件：InferenceRequest
    接收闭包结果：None
    提交闭包：supervise_inference(InferenceContext) -> Future<Output = ()>
    产出事件：None
    oneshot回复：Result<InferenceResponse, InferenceError>
    行为：InferenceRequest -> handle_inference。
```

# handler

## 函数

crate公开：
```text
handle_init_inference(world: &mut World, request: InitInference)
    行为：认领reply，检查agent实体；不存在ActiveInference则挂载current=None的组件，已存在不覆盖；组件可查询后回复Ok(())，失败回复InferenceError。

handle_inference(world: &mut World, request: InferenceRequest)
    行为：
        认领reply，检查完整Agent Entity、Working、当前turn及ActiveInference；已有未结束推理则拒绝。
        无有效turn返回Cancelled响应，不创建HTTP任务。
        从ModelRoutes选定路由，从Tools取得各alias路径；拒绝重复或不存在的alias。
        固定本次已注册webui发送器和对外身份投影，提交supervise_inference并将InferenceHandle存入ActiveInference。
        提交失败回复错误。
```

私有：
```text
supervise_inference(context: InferenceContext) -> Future<Output = ()>
    行为：
        blocking worker读取本次工具定义，校验消息及参数，构造DomainRequest。
        编码HTTP请求，建立有界mpsc::channel<ProviderFragment>并启动read_provider。
        运行receive_stream，以其累积器作为唯一内容结果来源。
        正常结束校验协议终态；取消时冻结接受门、关闭接收端并请求worker退出。
        等待worker真实join，释放HTTP及推流任务；清理失败返回CleanupFailed。
        用户中止且清理成功返回冻结的Cancelled结果；其他失败返回真实错误。
        通过原reply完成一次回复，不写历史、不执行工具、不重试。

read_provider(context: ProviderContext, sender: mpsc::Sender<ProviderFragment>) -> Future<Output = Result<(), InferenceError>>
    行为：执行HTTP并有界解析SSE，按顺序发送ProviderFragment；不向前端发送，不维护另一份最终正文；发送失败或取消后退出并释放连接。

receive_stream(context: StreamContext, receiver: mpsc::Receiver<ProviderFragment>) -> Future<Output = Result<AcceptedResponse, InferenceError>>
    行为：
        每轮使用biased select!优先检查取消，再等待一个分片，不批量排空。
        取得分片后在接受门短锁内复检关闭状态；允许才更新累积器并分配sequence。
        仅将已接受分片送往前端；网络发送不持锁、不阻塞接收循环。
        中止立即关闭接受门和receiver，丢弃未接受分片；不等待上游退出才冻结。
        正常和Cancelled结果都从同一累积器生成，worker退出结果不得补写正文。

push_stream(context: StreamDelivery, fragment: AcceptedDelta) -> Future<Output = ()>
    行为：向本次发送器快照有界发送正文或reasoning；剔除关闭及慢连接；发送失败不改变推理结果。

build_request(input: DomainRequest, route: ModelRoute) -> Result<ProviderHttpRequest, InferenceError>
    行为：按ApiFormat转换具体请求，再按Vendor添加选项、鉴权和端点；不猜测格式或厂商。
```

# types

## 类型

私有：
```text
InferenceContext：请求执行材料--原请求及reply、路由、工具路径、HTTP客户端、取消能力和推流投影
ProviderContext：上游材料--HTTP请求、格式解码器、取消能力及读取预算
StreamContext：接收材料--Arc<Mutex<StreamGate>>、取消能力及StreamDelivery
StreamDelivery：前端出口--固定发送器快照、请求ID、对外地址和不透明实例标识

StreamGate：本次流接受门
    closed: bool
    accumulator: ResponseAccumulator
    sequence: u64
    close(&mut self)
        行为：冻结本次累积内容，拒绝后续分片。

ProviderFragment：格式归一后的分片，私有枚举
    Content(String)
    Reasoning(String)
    ToolCallDelta { index: usize, id: Option<String>, name: Option<String>, arguments: String }
    Replay(serde_json::Value)
    Usage(InferenceUsage)
    Finished(InferenceStopReason)

AcceptedDelta：已接受的显示分片--sequence及Content或Reasoning
AcceptedResponse：接收器结果--累积消息和已接受协议终态
ResponseAccumulator：唯一响应累积器--正文、思考、工具调用、replay、usage及停止原因
FormatDecoder：SSE解析状态--跨字节缓冲、协议索引及完整结束校验，不另存最终正文

ApiFormat：格式，私有枚举
    ChatCompletions | Messages | Responses
Vendor：厂商，私有枚举
    OpenAi | Anthropic | DeepSeek | OpenRouter
ApiRequest：具体格式请求，私有枚举
    ChatCompletions(ChatCompletionsRequest)
    Messages(MessagesRequest)
    Responses(ResponsesRequest)
ProviderHttpRequest：HTTP请求--method、url、headers、body
```

# types pub

## 类型

公开：
```text
InferenceHandle：本次推理句柄，公开类型
    request_id: String
    turn: String
    cancel: CancellationToken
    stop_stream: Arc<dyn Fn() + Send + Sync>--本次StreamGate关闭函数，由inference_plugin构造
    task: JoinHandle<()>--supervise_inference任务句柄
    cancel(&self)
        行为：同步调用stop_stream冻结接受门，再触发cancel，不直接abort监督任务。

InferenceParameters：单次模型参数
    model: String--路由选择键
    temperature: Option<f32>
    max_output_tokens: Option<u32>
    top_p: Option<f32>
    stop: Vec<String>

ModelRoute：配置路由
    model: String--上游模型名
    provider: String
    api_type: String
    base_url: String
    api_key: String--不得写日志或对外投影
    context_window_tokens: u64
    thinking: Option<String>
    reasoning_effort: Option<String>

DomainRequest：拥有所有权的协议输入
    messages: Vec<InferenceMessage>
    tools: Vec<InferenceToolDefinition>
    config: InferenceParameters--编码前model替换为上游模型名

InferenceMessage：推理自身的消息契约，公开枚举
    System { content: String }
    User { content: String, images: Vec<InferenceImage> }
    Assistant { reasoning: Option<String>, content: Option<String>, tool_calls: Vec<InferenceToolCall>, replay: Option<serde_json::Value> }
    Tool { call_id: String, content: String, images: Vec<InferenceImage>, failed: bool }

InferenceToolDefinition：本次工具定义--name、description、input_schema
InferenceToolCall：模型调用--id、name、arguments
InferenceImage：图片--media_type、标准base64 data
InferenceUsage：用量--input_tokens、output_tokens、可选cache_hit_tokens

InferenceResponse：成功响应
    reasoning: Option<String>
    content: Option<String>
    tool_calls: Vec<InferenceToolCall>
    replay: Option<serde_json::Value>
    usage: Option<InferenceUsage>
    stop_reason: InferenceStopReason

InferenceStopReason：停止原因，公开枚举
    Completed | ToolCalls | Length | ContentFilter | Cancelled | Other(String)
```

# error

## 类型

公开：
```text
InferenceError：执行错误
    kind: InferenceErrorKind
    message: String
    impl Display + std::error::Error

InferenceErrorKind：错误类别，公开枚举
    InvalidRequest | AgentMissing | InferenceBusy | InvalidParameters | InvalidMessages | ToolMissing
    InvalidToolDefinition | ModelRouteMissing | InvalidModelRoute | UnsupportedFormat
    RequestFailed | ResponseStatus | ResponseInvalid | ResponseIncomplete | ResponseTooLarge
    Timeout | Cancelled | ReplyClosed | TaskPanicked | CleanupFailed
```

# 持有关系

```text
World
├── ModelRoutes: Resource
└── Agent Entity
    ├── Agent（agent_core）
    ├── ActiveInference（inference_core）
    │   └── Option<InferenceHandle>
    └── Tools（tool_resource_core）

supervise_inference
├── read_provider
│   └── mpsc::Sender<ProviderFragment>
├── receive_stream
│   ├── mpsc::Receiver<ProviderFragment>
│   └── StreamGate
│       └── ResponseAccumulator
├── 已接受分片的前端发送出口
└── 原请求oneshot回复端
```
