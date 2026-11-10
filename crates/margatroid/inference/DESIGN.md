# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名和直接行为。
System分别列出事件、闭包结果、提交闭包及oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
方法使用下列层级；--仅承载简短说明，执行逻辑写入行为。
method(args) -> xxx
    行为：
        xxx
```

# 板块定义

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

```text
-- 公开，定义于inference_core
ModelRoutes：模型配置，公开Resource
    entries: BTreeMap<String, ModelRoute>--私有
    get(&self, id: &str) -> Option<&ModelRoute>
        行为：
            借用模型路由，不输出凭据。
    impl Resource for ModelRoutes
```

```text
-- 公开，定义于inference_plugin
InferencePlugin：单次推理执行插件
    schedule: String
    routes: ModelRoutes
    open(path: &Path) -> Result<Self, InferenceError>
        行为：
            有界读取models.toml，完整校验provider、api_type及路由选项；
            失败不构造插件。
    impl Plugin for InferencePlugin
        build(self, app: &mut App)
            行为：
                检查运行设施，拒绝重复安装；
                schedule仅允许UPDATE或POST_UPDATE，拒绝PRE_UPDATE及无法证明顺序的自定义schedule；
                安装ModelRoutes及共享reqwest HTTP客户端，挂载init_inference_system和inference_system。不通过临时握手取得Tokio Handle，不在Resource保存执行器Handle；
                推理启动进入普通异步闭包。
```

# components

## 类型

```text
-- 公开
ActiveInference：该Agent的当前推理句柄，公开组件
    current: Option<InferenceHandle>--初始化为None，不放入ToolTasks
    impl Component for ActiveInference
```

# events

## 类型

```text
-- 公开
InitInference：推理句柄组件初始化请求，公开事件
    agent: Entity
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), InferenceError>>>>>
    impl Event for InitInference

InferenceRequest：推理请求，公开事件
    id: String
    agent: Entity
    turn: String
    messages: Vec<InferenceMessage>
    config: InferenceParameters
    cancel: CancellationToken
    reply: Arc<Mutex<Option<oneshot::Sender<Result<InferenceResponse, InferenceError>>>>>
    impl Event for InferenceRequest
```

# system

## 函数

```text
-- crate公开
init_inference_system(world: &mut World)
    处理事件：InitInference
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    oneshot回复：Result<(), InferenceError>
    行为：
        InitInference -> handle_init_inference。

inference_system(world: &mut World)
    处理事件：InferenceRequest
    接收闭包结果：Result<InferenceLaunchOutput, InferenceLaunchError>
    提交闭包：launch_inference(InferenceContext) -> Future<Output = Result<InferenceLaunchOutput, InferenceLaunchError>>
    产出事件：None
    oneshot回复：Result<InferenceResponse, InferenceError>
    行为：
        所在schedule严格晚于中止System所在schedule；
        InferenceRequest -> handle_inference；
        闭包启动结果只收口启动交接，InferenceCleanupFinished仍核对真实完成后才唤醒Driver。
```

# handler

## 函数

```text
-- crate公开
handle_init_inference(world: &mut World, request: InitInference)
    行为：
        认领reply，检查agent实体；
        不存在ActiveInference则挂载current=None的组件，已存在不覆盖；
        组件可查询后回复Ok(())，失败回复InferenceError。

handle_inference(world: &mut World, request: InferenceRequest)
    行为：
        认领reply，检查完整Agent Entity及Working；
        先检查当前turn，再检查ActiveInference及重叠推理。
        在System实际受理时检查Agent当前turn；
        已被中止清空或与request.turn不匹配，直接返回空assistant的Completed响应（正文、reasoning、tool_calls及usage均空），不创建HTTP或监督任务。
        从ModelRoutes选定路由；
        直接遍历当前Agent的Tools，以每条记录的alias、description及input_schema构造本次工具定义。
        在受理阶段校验消息及参数，构造DomainRequest并调用build_request完成HTTP编码。
        向闭包交付已编码请求；工具定义和工具名称集合不进入闭包，后续调用按当前Tools查找及校验参数。
        从client_core取得本次已注册webui的ClientSender快照，固定对外身份；
        同步登记尚未启动的共享InferenceHandle，防止异步提交窗口受理重叠请求；
        通过send_async_closure提交launch_inference。
        launch_inference在闭包内部创建监督任务并把JoinHandle交给同一共享句柄，不依赖全局执行器。ClientFrame::Text只携带已编码分片，不导入Axum消息、DTO插件或服务器框架发送器。
        提交失败回复错误。
```

```text
-- 私有
launch_inference(context: InferenceContext) -> Future<Output = Result<InferenceLaunchOutput, InferenceLaunchError>>
    行为：
        在普通异步闭包内启动supervise_inference，将真实JoinHandle写入已登记共享句柄；
        返回仅表示启动交接，不能解除ActiveInference并发限制。启动失败记录错误、完成原reply及启动失败通知，不保留一个永远未启动的句柄。

supervise_inference(context: InferenceContext) -> Future<Output = ()>
    行为：
        取出已编码HTTP请求，沿用受理时起算的deadline。
        建立有界mpsc::channel<ProviderFragment>并启动read_provider。
        完成响应时仅校验调用ID、名称和参数的结构，不校验名称是否已提供给模型。
        运行receive_stream，以其累积器作为唯一内容结果来源。
        正常结束校验协议终态；
        中止直接使用Agent上ActiveInference保存的当前取消句柄，控制本次分片接收闭包，先冻结接受门、关闭接收端并请求worker退出；
        不等工具或推理自然完成才取消。
        已确认的provider或流错误立即通过原reply交付一次；
        后台继续等待worker真实join，不提前释放ActiveInference。
        用户中止且清理成功返回冻结内容的Completed结果；
        清理预算耗尽及时返回CleanupFailed，原监督继续真实清理。
        全部真实清理结束发送绑定Agent、VM实例和request_id的私有完成事件；
        确认监督任务已结束后才唤醒当前Driver，未结束则下一帧复查，过期事件丢弃，不自动finish_turn。
        不写历史、不执行工具、不重试推理。

read_provider(context: ProviderContext, sender: mpsc::Sender<ProviderFragment>) -> Future<Output = Result<(), InferenceError>>
    行为：
        执行HTTP并有界解析SSE，按顺序发送ProviderFragment；
        不向前端发送，不维护另一份最终正文；
        发送失败或取消后退出并释放连接。

receive_stream(context: StreamContext, receiver: mpsc::Receiver<ProviderFragment>) -> Future<Output = Result<AcceptedResponse, InferenceError>>
    行为：
        每轮使用biased select!优先检查取消，再等待一个分片，不批量排空。
        取得分片后在接受门短锁内复检关闭状态；
        允许才更新累积器并分配sequence。
        仅将已接受分片送往前端；
        网络发送不持锁、不阻塞接收循环。
        中止立即关闭接受门和receiver，丢弃未接受分片；
        不等待上游退出才冻结。
        正常和中止的Completed结果都从同一累积器生成，worker退出结果不得补写正文。

push_stream(context: StreamDelivery, fragment: AcceptedDelta) -> Future<Output = ()>
    行为：
        向本次发送器快照有界发送正文或reasoning；
        剔除关闭及慢连接；
        发送失败不改变推理结果。

build_request(input: DomainRequest, route: ModelRoute) -> Result<ProviderHttpRequest, InferenceError>
    行为：
        按ApiFormat转换具体请求，再按Vendor添加选项、鉴权和端点；
        不猜测格式或厂商。
```

# types

## 类型

```text
-- 私有
InferenceContext：请求执行材料--原请求及reply、已编码http_request、deadline、HTTP客户端、取消能力和推流投影
ProviderContext：上游材料--HTTP请求、格式解码器、取消能力及读取预算
StreamContext：接收材料--Arc<Mutex<StreamGate>>、取消能力及StreamDelivery
StreamDelivery：前端出口--Vec<client_core::ClientSender>固定快照、请求ID、对外地址和不透明实例标识；只发送ClientFrame，不持有WebSocket或访问DTO连接表。
InferenceLaunchOutput：已将监督任务交给共享句柄的启动事实
InferenceLaunchError：InferenceError及原回复、共享句柄关联，实现From<AsyncTaskError>

StreamGate：本次流接受门
    closed: bool
    accumulator: ResponseAccumulator
    sequence: u64
    close(&mut self)
        行为：
            冻结本次累积内容，拒绝后续分片。

ProviderFragment：格式归一后的分片，私有枚举
    Content(String)
    Reasoning(String)
    ToolCallDelta { index: usize, id: Option<String>, name: Option<String>, arguments: String }
    Usage(InferenceUsage)
    Finished(InferenceStopReason)

AcceptedDelta：已接受的显示分片--sequence及Content或Reasoning
AcceptedResponse：接收器结果--累积消息和已接受协议终态
ResponseAccumulator：唯一响应累积器--正文、思考、工具调用、usage及停止原因
FormatDecoder：SSE解析状态--跨字节缓冲、协议索引及完整结束校验，不另存最终正文

ApiFormat：格式，私有枚举
    ChatCompletions | Messages | Responses
Vendor：厂商，私有枚举
    OpenAi | Anthropic | DeepSeek | OpenRouter
ApiRequest：具体格式请求，私有枚举
    ChatCompletions(ChatCompletionsRequest)
    Messages(MessagesRequest)
    Responses(ResponsesRequest)
ChatCompletionsRequest / MessagesRequest / ResponsesRequest：具体格式请求
    impl TryFrom<DomainRequest>或TryFrom<EncodeInput>
        行为：
            转换消息、工具及参数；
            在HTTP前校验目标格式与厂商能力，不经过通用JSON中间请求；
            Chat按EncodeInput能力映射reasoning_content，Messages/Responses仅编码其支持的独立字段，不把历史思考混入正文或构造签名块；
            只有reasoning且无正文/工具的历史若目标无法表达则明确失败，有正文或工具时保持它们原样转换。
EncodeInput：插件内格式编码材料--DomainRequest及本次只读能力，不进入领域core。
OpenAi<T> / Anthropic<T> / DeepSeek<T> / OpenRouter<T>：具体厂商handler
    T：具体ChatCompletionsRequest、MessagesRequest或ResponsesRequest，不是ApiRequest枚举变体。
    process(request: T) -> Result<ProviderHttpRequest, InferenceError>
        行为：
            只有支持矩阵中的具体组合实现process；
            以有类型字段处理厂商选项、鉴权和端点，附上同一组合选择的响应解码器。
ApiAdapter：显式组合分派，私有枚举
    OpenAiChat(OpenAi<ChatCompletionsRequest>)
    OpenAiResponses(OpenAi<ResponsesRequest>)
    AnthropicMessages(Anthropic<MessagesRequest>)
    DeepSeekChat(DeepSeek<ChatCompletionsRequest>)
    DeepSeekMessages(DeepSeek<MessagesRequest>)
    DeepSeekResponses(DeepSeek<ResponsesRequest>)
    OpenRouterChat(OpenRouter<ChatCompletionsRequest>)
    行为：
        只接受已登记的vendor与format组合，转换具体请求并调用对应process。
        不以通用VendorAdapter在JSON对象上修补差异。
ProviderHttpRequest：HTTP请求--method、url、headers、body及格式响应解码器
    行为：
        响应沿请求所选组合逆向归一，不再单独按配置猜解码器。
        分片仍进入唯一累积器。
```

# types pub

## 类型

```text
-- 公开
InferenceHandle：本次推理句柄，公开类型
    request_id: String
    turn: String
    cancel: CancellationToken
    stop_stream: Arc<dyn Fn() + Send + Sync>--本次StreamGate关闭函数，由inference_plugin构造
    task: Arc<InferenceTaskState>--共享启动状态、Mutex<Option<JoinHandle<()>>>及完成通知
    attach_task(&self, task: JoinHandle<()>) -> Result<(), InferenceError>
        行为：
            只允许启动交接一次，未交接前is_finished始终为false；
            启动失败显式记录失败完成，不能误当真实运行任务退出。
    is_finished(&self) -> bool
        行为：
            区分未启动、运行中、启动失败和真实join完成，不以slot为None推断完成。
    wait_finished(&self) -> Future<Output = Result<(), InferenceError>>
        行为：
            Agent退出流程等待真实监督任务结束或明确启动失败，不跨await持锁，不通过取走句柄使其他读者误判完成。
    cancel(&self)
        行为：
            同步调用stop_stream冻结接受门，再触发cancel，不直接abort监督任务。

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
    User { content: String, images: Vec<InferenceImage> }--JSON字段名为data，无images别名
    Assistant { reasoning: Option<String>, content: Option<String>, tool_calls: Vec<InferenceToolCall> }
    Tool { call_id: String, content: String, images: Vec<InferenceImage>, failed: bool }--JSON字段名为data

InferenceToolDefinition：本次工具定义--name、description、input_schema
InferenceToolCall：模型调用--id、name、arguments
InferenceImage：图片--media_type、标准base64 data
InferenceUsage：用量--input_tokens、output_tokens、可选cache_hit_tokens

InferenceResponse：成功响应
    reasoning: Option<String>
    content: Option<String>
    tool_calls: Vec<InferenceToolCall>
    usage: Option<InferenceUsage>
    stop_reason: InferenceStopReason

InferenceStopReason：停止原因，公开枚举
    Completed | ToolCalls | Length | ContentFilter | Other(String)
```

# error

## 类型

```text
-- 公开
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
