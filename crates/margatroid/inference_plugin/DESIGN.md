# InferencePlugin

## 类型

公开：
```text
ModelId：模型路由ID，公开元组结构体--AgentImage引用的稳定名称，通常使用具体模型名，不等于Provider请求值
    0: String--非空逻辑名称，私有
    new(value: impl Into<String>) -> Result<Self, InferenceError>
        构造名称：公开关联函数，验证value非空且不包含控制字符
    as_str(&self) -> &str
        获取名称：公开方法，返回内部字符串引用

InferenceParameters：统一推理参数，公开结构体--只保存跨Provider有稳定语义的参数
    temperature: Option<f32>--采样温度
    max_output_tokens: Option<u32>--最大输出token数
    top_p: Option<f32>--核采样参数
    stop: Vec<String>--停止序列
    new(temperature: Option<f32>, max_output_tokens: Option<u32>, top_p: Option<f32>, stop: Vec<String>) -> Self
        构造参数：公开关联函数，原样保存参数；业务范围由validate统一检查
    temperature(&self) -> Option<f32>
        取得温度：公开方法
    max_output_tokens(&self) -> Option<u32>
        取得最大输出量：公开方法
    top_p(&self) -> Option<f32>
        取得核采样参数：公开方法
    stop(&self) -> &[String]
        取得停止序列：公开方法
    validate(&self) -> Result<(), InferenceError>
        验证参数：crate公开方法，检查数值范围、停止序列数量与长度
    impl Default for InferenceParameters
        Default：公开trait实现，全部可选参数为空且stop为空

AgentInferenceSnapshot：Agent实例推理快照，公开组件--启动时从中立AgentImageModelConfig转换，不随AgentImage编辑自动更新
    model: ModelId--实际使用的逻辑模型ID
    parameters: InferenceParameters--实际使用的参数
    workspace: Entity--所属Workspace Entity，用于查询项目级模型路由
    source_image: Entity--来源AgentImage Entity，仅用于追踪，不在推理时重新读取
    model(&self) -> &ModelId
        取得逻辑模型：公开方法
    parameters(&self) -> &InferenceParameters
        取得推理参数：公开方法
    workspace(&self) -> Entity
        取得Workspace：公开方法
    source_image(&self) -> Entity
        取得来源镜像：公开方法
    impl Clone for AgentInferenceSnapshot
        Clone：公开trait实现，允许Workspace为Agent实例准备独立推理配置
    impl Component for AgentInferenceSnapshot
        Component：公开trait实现

WorkspaceModelRoutes：Workspace模型路由，公开组件--挂在Workspace Entity上的项目级路由覆盖
    routes: HashMap<ModelId, ConfiguredModelRoute>--从<project>/.margatroid/models.toml编译的项目级路由，私有
    get(&self, id: &ModelId) -> Option<ConfiguredModelRoute>
        取得项目路由：crate公开方法，克隆实际模型名称和Adapter共享引用
    len(&self) -> usize
        取得路由数量：公开方法
    is_empty(&self) -> bool
        检查空路由：公开方法
    impl Component for WorkspaceModelRoutes
        Component：公开trait实现

InferenceCommand：推理命令，公开事件--携带已处理的messages、AgentPlugin收集的工具定义和稳定Agent ID
    id: String--调用方生成的请求ID，与agent共同构成并发响应路由
    agent: Entity--发起本次推理的AgentInstance Entity
    agent_id: String--AgentPlugin提供的稳定逻辑ID，用于构造外部流式消息
    messages: Vec<Message>--本次请求的完整消息快照
    tools: Vec<ToolDefinition>--AgentPlugin遍历动态可见资源并逐个构造后收集的工具定义
    impl Event for InferenceCommand
        Event：公开trait实现

StopReason：停止原因，公开枚举--描述Provider结束本次响应的原因
    Completed--正常完成
    ToolCalls--返回一个或多个工具调用
    Length--达到长度限制
    ContentFilter--被内容策略终止
    Other(String)--Provider返回的其他稳定文本

TokenUsage：Token用量，公开结构体--Provider没有返回时整个字段为空
    input_tokens: u64--输入token数
    output_tokens: u64--输出token数
    total_tokens: u64--总token数

InferenceResponse：统一推理响应，公开结构体--Provider响应累积器完成后交给发布System的协议无关产物
    message: Message--必须是Assistant消息
    stop_reason: StopReason--停止原因
    usage: Option<TokenUsage>--可选用量

ReloadModelRoutes：重载模型路由，公开事件--请求InferencePlugin重新加载主目录全局models.toml
    id: String--调用方生成的请求ID
    impl Event for ReloadModelRoutes
        Event：公开trait实现

ModelRoutesReloaded：模型路由重载结果，公开结构体--成功换入的新全局路由表摘要
    route_count: usize--新路由数量

ReloadModelRoutesResult：重载模型路由结果，公开事件--与ReloadModelRoutes按id配对
    id: String--原请求ID
    result: Result<ModelRoutesReloaded, InferenceError>--重载结果
    impl Event for ReloadModelRoutesResult
        Event：公开trait实现

InferenceErrorKind：推理错误分类，公开枚举
    InvalidModelId
    ConfigPathUnavailable
    ConfigReadFailed
    ConfigDecodeFailed
    DuplicateModelId
    InvalidModelRoute
    UnsupportedApiType
    InvalidCommand
    AgentNotAlive
    InferenceSnapshotMissing
    ModelRouteNotFound
    InvalidParameters
    InvalidMessages
    InvalidToolDefinitions
    UnsupportedInput
    RequestBuildFailed
    RequestFailed
    ResponseStatus
    ResponseDecodeFailed
    ResponseEncodeFailed
    ResponseIncomplete
    TaskPanicked

InferenceError：推理错误，公开结构体--不保存API key、Authorization header、完整请求或完整响应
    kind: InferenceErrorKind--错误分类
    message: String--有界诊断文本
    status: Option<u16>--Provider HTTP状态码
    new(kind: InferenceErrorKind, message: impl Into<String>) -> Self
        构造错误：公开关联函数，保存分类和有界描述，不设置HTTP状态码
    with_status(kind: InferenceErrorKind, status: Option<u16>, message: impl Into<String>) -> Self
        构造带状态错误：公开关联函数，将描述按UTF-8边界截断到512字节
    panic(self) -> !
        终止配置：crate私有方法，以Display文本触发panic
    kind(&self) -> InferenceErrorKind
        获取分类：公开方法，返回kind
    message(&self) -> &str
        获取描述：公开方法，返回message引用
    status(&self) -> Option<u16>
        获取状态码：公开方法，返回可选HTTP状态码
    impl fmt::Display for InferenceError
        Display：公开trait实现，只输出kind、status和有界message
    impl std::error::Error for InferenceError
        Error：公开trait实现

ProviderInput<'a>：Provider输入视图，公开结构体--ProviderAdapter组装请求时只读借用
    model: &'a str--模型名称
    parameters: &'a InferenceParameters--推理参数
    messages: &'a [Message]--完整统一消息
    tools: &'a [ToolDefinition]--允许的工具定义
    new(model: &'a str, parameters: &'a InferenceParameters, messages: &'a [Message], tools: &'a [ToolDefinition]) -> Self
        构造输入：私有关联函数，由InferencePlugin准备阶段创建
    model(&self) -> &str
        取得实际模型：公开方法
    parameters(&self) -> &InferenceParameters
        取得参数：公开方法
    messages(&self) -> &[Message]
        取得消息：公开方法
    tools(&self) -> &[ToolDefinition]
        取得工具定义：公开方法

ProviderRouteInput<'a>：Provider路由视图，公开结构体--ProviderAdapterFactory创建已配置Adapter时只读借用
    provider: Option<&'a str>--可选供应商名称，不参与路由查找
    base_url: &'a reqwest::Url--完整API基础地址
    api_key: &'a str--鉴权密钥，不得保存到日志、Event或Error
    provider(&self) -> Option<&str>
        取得供应商元数据：公开方法
    base_url(&self) -> &reqwest::Url
        取得API基础地址：公开方法
    api_key(&self) -> &str
        取得鉴权密钥：公开方法，只允许Adapter构造请求时使用

ProviderHttpRequest：Provider HTTP请求，公开结构体--允许Adapter构造但不允许业务读取鉴权内容
    method: reqwest::Method--HTTP方法，私有
    url: reqwest::Url--请求地址，私有
    headers: reqwest::header::HeaderMap--请求头，私有，可能含secret
    body: Vec<u8>--请求体，私有
    new(method: reqwest::Method, url: reqwest::Url, headers: HeaderMap, body: Vec<u8>) -> Self
        构造请求：公开关联函数，转移完整HTTP请求所有权

ProviderAdapter：Provider协议适配器，公开trait--组装一种模型API请求并解析其响应的核心接口
    继承：Send + Sync + 'static
    build_request(&self, input: ProviderInput<'_>) -> Result<ProviderHttpRequest, InferenceError>
        组装请求：公开方法，将统一输入转换为Provider HTTP请求
    begin_response(&self, status: reqwest::StatusCode, headers: &HeaderMap) -> Result<Box<dyn ProviderResponseAccumulator>, InferenceError>
        开始响应：公开方法，检查状态与响应头并创建当前响应的独立累积器

ProviderAdapterFactory：Provider协议工厂，公开trait--将一条路由配置构造为已绑定端点和密钥的Adapter
    继承：Send + Sync + 'static
    build(&self, route: ProviderRouteInput<'_>) -> Result<Arc<dyn ProviderAdapter>, InferenceError>
        构造适配器：公开方法，验证路由并且不向错误暴露api_key

ProviderResponseAccumulator：Provider响应累积器，公开trait--每个请求独占，负责协议分帧与统一结果构造
    继承：Send + 'static
    push(&mut self, chunk: &[u8]) -> Result<Vec<String>, InferenceError>
        推入分片：公开方法，允许任意网络分片边界，返回本次新解析出的可展示文本片段
        行为：工具调用ID、名称和参数片段只在累积器内部保存，不返回给流式通道
    finish(self: Box<Self>) -> Result<(InferenceResponse, Vec<String>), InferenceError>
        完成响应：公开方法，解析缓冲区中的尾行，验证协议结束与工具调用完整性，并返回统一响应和尾行中新解析出的可展示文本片段

InferencePlugin：推理插件，公开结构体
    schedule: String--命令准备、异步提交与结果发布所属Schedule，私有
    config_path: PathBuf--主目录全局模型路由表路径，私有
    adapter_factories: HashMap<String, ErasedProviderAdapterFactory>--api_type到协议工厂的映射，私有
    new() -> Self
        构造插件：公开关联函数，使用默认Schedule、默认配置路径和openai协议工厂
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，使用schedule替换默认Schedule并返回自身
    with_config_path(mut self, path: impl Into<PathBuf>) -> Self
        指定全局路由表：公开方法，使用path替换默认主目录配置路径并返回自身
    with_api_type<Factory>(mut self, api_type: impl Into<String>, factory: Factory) -> Self
        注册协议工厂：公开泛型方法，为路由表中的api_type扩展Provider协议
        约束：Factory: ProviderAdapterFactory
        行为：api_type非空且未重复时插入工厂并返回自身，否则终止配置
    impl Default for InferencePlugin
        Default：公开trait实现，使用RuntimePlugin::PRE_UPDATE、~/.margatroid/models.toml和内置openai协议工厂
        default() -> Self
            构造默认插件：调用new
    impl Plugin for InferencePlugin
        Plugin：公开trait实现，加载模型路由表并安装路由重载、推理处理所需的Resource和System

WorldInferenceExt：World推理扩展，公开trait--提供全局路由重载与Workspace项目级路由加载入口
    reload_model_routes(&self, id: impl Into<String>)
        重载路由：公开方法，使用id发送ReloadModelRoutes
        行为：调用app_runtime_plugin::WorldEventExt::send_event并唤醒Runtime
    load_workspace_model_routes(&mut self, workspace: Entity, project_root: &Path) -> Result<usize, InferenceError>
        加载Workspace路由：公开方法，从project_root/.margatroid/models.toml加载项目级路由
        行为：文件存在时编译并将WorkspaceModelRoutes插入workspace；不存在时移除原组件并返回0
    build_agent_inference_snapshot(
        &self,
        workspace: Entity,
        source_image: Entity,
        config: &AgentImageModelConfig,
    ) -> Result<AgentInferenceSnapshot, InferenceError>
        构建实例推理快照：公开方法，把AgentImage中立模型配置转换为经过验证的实例快照
        行为：
            将config.model构造为ModelId
            将config.parameters复制为InferenceParameters并验证业务范围
            按Workspace项目级、全局顺序确认ModelId存在可用路由
            返回记录workspace和source_image的AgentInferenceSnapshot
            不向World插入组件，不创建AgentInstance
    impl WorldInferenceExt for World
        WorldInferenceExt for World：公开trait实现

OpenAiAdapterFactory：OpenAI兼容协议工厂，公开单元结构体
    new() -> Self
        构造工厂：公开关联函数
    impl Default for OpenAiAdapterFactory
        Default：公开trait实现，返回无状态工厂
        default() -> Self
            构造默认工厂：返回单元结构体
    impl ProviderAdapterFactory for OpenAiAdapterFactory
        ProviderAdapterFactory：公开trait实现，验证API key并创建绑定路由的OpenAiAdapter
```

crate公开：
```text
ErasedProviderAdapter：擦除Provider适配器，crate公开类型别名--等于Arc<dyn ProviderAdapter>

ErasedProviderAdapterFactory：擦除Provider协议工厂，crate公开类型别名--等于Arc<dyn ProviderAdapterFactory>

ConfiguredModelRoute：已配置模型路由，crate公开结构体--逻辑模型ID对应的实际模型和已配置Adapter
    model: String--发送给实际API的模型名称
    adapter: ErasedProviderAdapter--已绑定base_url、api_key与供应商元数据的Adapter

GlobalModelRoutes：全局模型路由，crate公开Resource--主目录全局默认路由表的运行时表示
    path: PathBuf--主目录全局模型路由表路径
    factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>--api_type到协议工厂的不可变映射
    routes: HashMap<ModelId, ConfiguredModelRoute>--逻辑模型ID到实际路由的映射
    load(path: PathBuf, factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>) -> Result<Self, InferenceError>
        加载注册表：crate公开关联函数，读取并编译path指向的全局模型路由表
    reload(&mut self) -> Result<usize, InferenceError>
        重载注册表：crate公开方法，从path重新读取并用编译结果直接替换routes
        行为：成功返回新路由数量；配置无法编译时返回错误
    get(&self, id: &ModelId) -> Option<ConfiguredModelRoute>
        取得路由：crate公开方法，克隆实际模型名称和Adapter共享引用
    impl Resource for GlobalModelRoutes
        Resource：crate公开trait实现

InferenceHttpClient：HTTP客户端Resource，crate公开结构体--统一连接池、TLS和请求超时配置
    client: reqwest::Client--reqwest客户端
    new() -> Result<Self, InferenceError>
        构造客户端：crate公开关联函数，使用安全默认值创建Client
    impl Resource for InferenceHttpClient
        Resource：crate公开trait实现

```

私有：
```text
ModelRouteDocument：模型路由表文档，私有结构体--models.toml顶层反序列化对象
    models: Vec<ModelRouteConfig>--按文件顺序保存的模型路由

ModelRouteConfig：模型路由配置，私有结构体--一个逻辑模型ID对应的实际推理端点
    id: String--逻辑模型ID
    model: String--实际API模型名称
    provider: Option<String>--可选供应商名称，不参与路由查找
    base_url: String--包含scheme的完整API基础地址
    api_key: String--鉴权密钥
    api_type: String--选择ProviderAdapterFactory的协议名称

InferenceRoute：推理路由，私有结构体--在内部异步边界始终携带，保证多Agent并发结果可配对
    id: String--请求ID
    agent: Entity--AgentInstance Entity

PreparedInference：已准备推理任务，私有结构体--主线程已完成所有World读取和请求组装
    route: InferenceRoute--响应路由
    client: reqwest::Client--从InferenceHttpClient克隆的共享HTTP客户端
    request: ProviderHttpRequest--完整HTTP请求
    adapter: ErasedProviderAdapter--对应协议适配器
    agent_id: String--稳定Agent逻辑ID
    senders: Vec<WebSocketSender>--prepare_inference_system按全局streaming_member_messages目标解析并固定的本轮连接发送器
    impl Event for PreparedInference
        Event：私有trait实现

InferenceTaskOutput：异步推理任务输出，私有结构体--无论成功失败都保留原路由
    route: InferenceRoute--响应路由
    result: Result<InferenceResponse, InferenceError>--Provider执行结果

InferenceTaskError：异步监督错误，私有结构体--表示AsyncRuntime在推理处理器外层取消任务
    source: AsyncTaskError--异步运行时错误
    impl From<AsyncTaskError> for InferenceTaskError
        From<AsyncTaskError>：私有trait实现，满足AppAsyncExt::add_async_system的错误约束
        from(source: AsyncTaskError) -> Self
            转换监督错误：保存source

AgentMessageDeltaFrame<'a>：成员流式消息帧，私有结构体--直接序列化给WebSocket发送终端，不进入事件队列
    message_type: &'static str--固定为agent.message.delta
    id: &'a str--推理请求ID
    agent: &'a str--稳定Agent ID
    content: String--本次新增文本

OpenAiAdapter：OpenAI兼容协议适配器，私有结构体--保存单条路由的端点和密钥
    base_url: reqwest::Url--API基础地址
    api_key: String--鉴权密钥
    impl ProviderAdapter for OpenAiAdapter
        ProviderAdapter：私有trait实现，构造流式chat/completions请求并创建OpenAiAccumulator

OpenAiRequest：OpenAI兼容请求正文，私有结构体
    model: String--实际模型名
    messages: Vec<serde_json::Value>--协议消息数组
    tools: Vec<serde_json::Value>--函数工具定义
    stream: bool--固定启用流式响应
    temperature: Option<f32>--采样温度
    max_tokens: Option<u32>--最大输出量
    top_p: Option<f32>--核采样参数
    stop: Vec<String>--停止序列
    from_input(input: ProviderInput<'_>) -> Self
        转换输入：私有关联函数，将统一消息、参数和工具转换为OpenAI兼容请求

OpenAiAccumulator：OpenAI兼容响应累积器，私有结构体--按SSE行解析并累积Assistant响应
    buffer: Vec<u8>--未形成完整行的网络字节
    content: String--完整文本内容
    tool_calls: Vec<OpenAiToolCallBuilder>--按index累积的工具调用
    stop_reason: Option<StopReason>--Provider停止原因
    usage: Option<TokenUsage>--Token用量
    saw_choice: bool--是否收到过choice
    done: bool--是否收到[DONE]
    consume_line(&mut self, line: &[u8]) -> Result<Vec<String>, InferenceError>
        消费响应行：私有方法，忽略空行和注释，解析data载荷或DONE标记
    consume_chunk(&mut self, chunk: OpenAiChunk) -> Result<Vec<String>, InferenceError>
        消费协议分片：私有方法，累积文本、工具调用、停止原因和用量
    impl ProviderResponseAccumulator for OpenAiAccumulator
        ProviderResponseAccumulator：私有trait实现，处理任意网络分片边界并完成统一响应

OpenAiToolCallBuilder：OpenAI工具调用临时槽，私有结构体
    id: String--分片拼接的调用ID
    name: String--分片拼接的工具名
    arguments: String--分片拼接的原始参数

OpenAiChunk：OpenAI响应分片，私有反序列化结构体
    choices: Vec<OpenAiChoice>--响应候选
    usage: Option<OpenAiUsage>--可选用量

OpenAiChoice：OpenAI响应候选，私有反序列化结构体
    delta: Option<OpenAiDelta>--流式增量
    message: Option<OpenAiDelta>--兼容端点可能返回的完整消息
    finish_reason: Option<String>--停止原因

OpenAiDelta：OpenAI消息增量，私有反序列化结构体
    content: Option<String>--文本增量
    tool_calls: Option<Vec<OpenAiToolCallDelta>>--工具调用增量

OpenAiToolCallDelta：OpenAI工具调用增量，私有反序列化结构体
    index: Option<usize>--工具调用槽位
    id: Option<String>--调用ID片段
    function: Option<OpenAiFunctionDelta>--函数片段

OpenAiFunctionDelta：OpenAI函数增量，私有反序列化结构体
    name: Option<String>--函数名片段
    arguments: Option<String>--参数片段

OpenAiUsage：OpenAI用量，私有反序列化结构体
    input_tokens: u64--输入Token，兼容prompt_tokens别名
    output_tokens: u64--输出Token，兼容completion_tokens别名
    total_tokens: u64--总Token
```

## 函数

私有：
```text
default_config_path() -> PathBuf
    默认配置路径：私有函数，返回~/.margatroid/models.toml；主目录不可取得时回退相对路径

load_model_routes(path: &Path, factories: &HashMap<String, ErasedProviderAdapterFactory>) -> Result<HashMap<ModelId, ConfiguredModelRoute>, InferenceError>
    加载模型路由：私有函数，Plugin构建期从path同步读取并编译完整模型路由表
    行为：有界读取UTF-8 TOML并调用compile_model_routes；此时Runtime尚未进入循环

compile_model_routes(source: &str, factories: &HashMap<String, ErasedProviderAdapterFactory>) -> Result<HashMap<ModelId, ConfiguredModelRoute>, InferenceError>
    编译模型路由：私有函数，解析、验证并构造模型路由映射
    行为：
        有界反序列化source为ModelRouteDocument
        路由表为空时返回InvalidModelRoute
        按文件顺序编译每个ModelRouteConfig
        验证id、model、api_key和api_type非空
        将id构造为ModelId，重复id返回DuplicateModelId
        将base_url解析为reqwest::Url，只允许http或https
        根据api_type取得ProviderAdapterFactory，不存在时返回UnsupportedApiType
        构造ProviderRouteInput并调用Factory::build
        将id、实际model与Adapter组装为ConfiguredModelRoute插入路由映射
        返回完整路由映射

require_workspace(world: &World, workspace: Entity) -> Result<(), InferenceError>
    检查Workspace：私有函数，确认Entity存活且具有WorkspaceIdentity

model_is_routable(world: &World, workspace: Entity, model: &ModelId) -> bool
    检查路由：私有函数，按Workspace项目级、全局顺序确认模型存在

reload_model_routes_system(world: &mut World)
    重载模型路由：私有System，读取ReloadModelRoutes并同步重载全局默认路由表
    行为：对每个ReloadModelRoutes依次执行
        id为空时立即发送InvalidCommand
        调用GlobalModelRoutes::reload
        成功时发送ReloadModelRoutesResult::Ok并携带route_count
        失败时发送ReloadModelRoutesResult::Err
        重载是低频管理操作，允许同步文件读取与编译阻塞当前帧

prepare_inference_system(world: &mut World)
    准备推理：私有System，读取InferenceCommand并在主线程生成PreparedInference
    行为：对每个Command依次执行
        验证id非空、agent存活且messages结构合法
        从agent读取AgentInferenceSnapshot
        直接使用command.tools；InferencePlugin不从Agent读取或推导另一份工具列表
        从snapshot.workspace读取WorkspaceModelRoutes并按snapshot.model查询
        项目级未找到时从GlobalModelRoutes查询全局默认路由
        构造ProviderInput借用route.model、snapshot.parameters、messages和tools
        调用Adapter::build_request
        任一检查或组装失败时立即发送AgentFailure { id, agent, kind: Inference, message }
        成功时调用WorldAsyncExt::send_async_event发送PreparedInference

prepare_inference(world: &World, command: InferenceCommand) -> Result<PreparedInference, InferenceError>
    准备单次推理：私有函数，完成Command验证、路由解析、请求构造、客户端克隆和流式发送器解析

resolve_websocket_targets(world: &World, target: &WebSocketMessageTarget) -> Vec<WebSocketSender>
    解析流式目标：私有函数，从WebSocketConnections取得目标当前匹配的发送器快照

execute_prepared_inference(prepared: PreparedInference) -> Result<InferenceTaskOutput, InferenceTaskError>
    执行推理：私有异步函数，发送HTTP请求、直接向固定WebSocket发送器转发文本并累积完整响应
    行为：
        始终保留prepared.route
        在panic捕获边界内使用prepared.client发送prepared.request
        调用Adapter::begin_response创建独占累积器
        异步读取response.bytes_stream
        每取得一段bytes就调用累积器::push
        将push返回的文本片段与route.id、agent_id包装为ServerMessage::AgentMessageDelta并序列化为WebSocketMessage
        为每个分片使用prepared.senders构造WebSocketMessageSender并await send
        发送器为空或连接已关闭时停止对应外部转发，不中止后端推理与累积
        流结束后调用累积器::finish
        将成功响应或任意InferenceError包装为InferenceTaskOutput
        Provider Future或Adapter panic时使用已保留的route返回TaskPanicked
        AsyncRuntime取消整个处理器时返回InferenceTaskError，此路径无法继续发布业务事件

publish_inference_output_system(world: &mut World)
    发布结果：私有System，将异步推理输出直接发布为共享AgentMessage或AgentFailure
    行为：
        成功取得InferenceTaskOutput时保留route.id和route.agent
        result为Ok时确认response.message是Message::Assistant
            tool_calls为空时赋予MessageIntent::CompleteTurn
            tool_calls非空时赋予MessageIntent::DispatchToolCalls
            发送AgentMessage { id, agent, message, intent }
        result为Err时提取安全有界描述并发送AgentFailure { id, agent, kind: Inference, message }
        取得InferenceTaskError时写入system log；该错误只会在Runtime取消任务等无法继续运行的路径出现
        不修改Agent的messages、不执行工具；只由当前消息来源赋予意图

validate_messages(messages: &[Message]) -> Result<(), InferenceError>
    验证消息：私有函数，检查消息数量、总字节上限及ToolCall配对所需字段
    行为：
        messages为空时返回InvalidMessages
        Assistant content为空且tool_calls为空时返回InvalidMessages
        ToolCall的id、name为空时返回InvalidMessages
        Tool Message的tool_call_id为空时返回InvalidMessages
        不要求Tool Message必须紧邻Assistant，但保持输入顺序不变

validate_tools(tools: &[ToolDefinition]) -> Result<(), InferenceError>
    验证工具：私有函数，检查名称唯一、描述有界且input_schema是合法对象Schema

send_provider_request(client: &reqwest::Client, request: ProviderHttpRequest) -> Result<reqwest::Response, InferenceError>
    发送请求：私有异步函数，将ProviderHttpRequest转换为reqwest Request并发送
    行为：设置method、url、headers和body；网络错误转换为RequestFailed并保留错误类别与最深层系统原因
    安全：错误只包含去除凭据和查询参数的endpoint，不记录header、API key和请求body

safe_endpoint(url: &Url) -> String
    安全端点：私有函数，只保留origin，移除用户凭据、path、查询参数与fragment

summarize_reqwest_error(error: &reqwest::Error) -> String
    汇总传输错误：私有函数，映射连接、超时、body、decode和request类别并保留最深层source

provider_error_detail(body: &[u8]) -> Option<String>
    提取Provider错误：私有函数，优先读取error.message、error、message或detail字符串，否则使用有界正文
    安全：移除控制字符、折叠空白，最终仍受InferenceError的512字节限制

single_line(value: &str) -> String
    单行化：私有函数，将控制字符替换为空格并折叠空白

read_bounded_body(response: reqwest::Response, limit: usize) -> Vec<u8>
    有界读取响应：私有异步函数，最多读取limit字节错误正文

run_provider(prepared: PreparedInference) -> Result<InferenceResponse, InferenceError>
    驱动Provider：私有异步函数，发送请求、检查状态、驱动累积器并直接转发文本分片

send_stream_delta(senders: &[WebSocketSender], id: &str, agent: &str, content: String) -> Result<(), InferenceError>
    发送流式文本：私有异步函数，序列化AgentMessageDeltaFrame并通过WebSocketMessageSender直接发送

openai_message(message: &Message) -> serde_json::Value
    转换OpenAI消息：私有函数，将统一Message映射为兼容协议JSON

parse_stop_reason(value: &str) -> StopReason
    解析停止原因：私有函数，映射已知OpenAI原因并保留未知文本
```

## 逻辑

```text
安装：
    app.add_plugin(RuntimePlugin)
        -> app.add_plugin(AsyncRuntimePlugin)
        -> app.add_plugin(AgentImageLoaderPlugin)
        -> app.add_plugin(InferencePlugin)
        -> 确认AgentImageLoaderPlugin已经安装
        -> 检查指定Schedule存在
        -> 解析~/.margatroid/models.toml或用户指定的路由表
        -> 按api_type使用ProviderAdapterFactory编译每条路由
        -> 插入进程内唯一的GlobalModelRoutes Resource
        -> 插入InferenceHttpClient
        -> 在指定Schedule挂载reload_model_routes_system
        -> 在指定Schedule挂载prepare_inference_system
        -> 在指定Schedule通过add_async_system挂载PreparedInference处理器
        -> 在指定Schedule挂载publish_inference_output_system

AgentImage启动：
    Workspace启动逻辑调用load_workspace_model_routes(workspace, project_root)
        -> 读取<project>/.margatroid/models.toml
        -> 文件存在时编译为WorkspaceModelRoutes并挂到Workspace Entity
        -> 文件不存在时Workspace仅使用全局默认路由
    Workspace启动逻辑读取AgentImage Entity的AgentImageModelConfig
        -> 调用world.build_agent_inference_snapshot(workspace, source_image, config)
        -> InferencePlugin构造ModelId和InferenceParameters并验证业务规则
        -> InferencePlugin按项目级、全局顺序验证ModelId存在
        -> WorkspacePlugin把返回的AgentInferenceSnapshot挂到新AgentInstance Entity
    运行中的InferencePlugin只读AgentInferenceSnapshot
    AgentImageModelConfig变化不会影响已启动AgentInstance

重载模型路由：
    world.reload_model_routes(id)
        -> 发送ReloadModelRoutes并唤醒Runtime
    reload_model_routes_system
        -> 同步重新读取全局models.toml
        -> 完整解析、验证并直接替换GlobalModelRoutes.routes
        -> 发送ReloadModelRoutesResult
    所有命中该全局ModelId且没有项目级覆盖的AgentInstance从下一次推理起一起切换
    项目级同名ModelId不受全局同名路由重载影响

发起推理：
    Agent核心克隆当前完整messages
        -> 遍历AgentDynamicVisibility.resources
        -> 逐个调用ToolPlugin.resolve_tool并收集ToolDefinition
        -> 读取AgentIdentity并send_event(InferenceCommand { id, agent, agent_id, messages, tools })
    prepare_inference_system
        -> 读取AgentInferenceSnapshot并使用command.tools
        -> 根据snapshot.workspace查询WorkspaceModelRoutes
        -> 项目级未找到snapshot.model时查询全局GlobalModelRoutes
        -> 读取全局配置的streaming_member_messages目标并通过WebSocketConnections解析为固定Vec<WebSocketSender>
        -> Adapter把统一Message、工具和实际模型配置组装为ProviderHttpRequest
        -> 克隆InferenceHttpClient内的reqwest::Client
        -> 将command.agent_id和固定senders写入PreparedInference并send_async_event
    异步System取得PreparedInference
        -> 发送HTTP请求
        -> 按网络分片持续喂给ProviderResponseAccumulator
        -> 将可展示文本片段包装并通过WebSocketMessageSender直接发送
        -> finish时先发送缓冲区尾行产生的可展示文本，再返回完整响应
        -> 工具调用片段只在累积器内部组装
        -> 响应结束后返回InferenceTaskOutput
    publish_inference_output_system
        -> 成功时发送AgentMessage { id, agent, Message::Assistant, intent }
        -> 失败时发送AgentFailure { id, agent, kind: Inference, message }

流式响应：
    文本片段只用于前端实时展示，通过WebSocketMessageSender直接发送
    文本片段不进入ECS事件队列，不直接写入Agent messages
    全局streaming_member_messages目标在prepare_inference_system中解析并固定，本轮中途新增连接从下一轮开始接收
    Provider可能按index分多次发送工具调用ID、名称和arguments片段
    ProviderResponseAccumulator按index维护独立临时槽位
    Text增量按到达顺序累积为Assistant content
    ToolCall arguments只拼接原始文本，不在分片阶段解析JSON
    finish时验证每个ToolCall的id、name和arguments完整
    最终生成Message::Assistant { content, tool_calls }

Agent收到结果：
    AgentMessage
        -> Assistant没有tool_calls时intent为CompleteTurn
        -> Assistant有tool_calls时intent为DispatchToolCalls
        -> AgentPlugin记入消息，然后执行来源已经赋予的intent
    AgentFailure
        -> 失败如何影响AgentStatus由后续Agent消息契约确定
    InferencePlugin不直接修改messages

Provider边界：
    ModelId只是逻辑路由键，不推断或限制实际provider与model
    provider是可选元数据，api_type才决定ProviderAdapterFactory
    ProviderAdapter可以采用OpenAI兼容JSON、其他JSON格式或自定义流式帧
    InferencePlugin只负责通用HTTP发送与分片驱动
    Adapter负责字段映射、SSE/JSON分帧、finish reason、usage和tool call差异
    Adapter不能读取World、Agent Entity组件或修改messages

错误与安全边界：
    全局models.toml只从daemon主目录或用户显式指定路径读取
    项目级models.toml只从Workspace的规范化project_root/.margatroid目录读取
    models.toml只描述Provider路由，不保存WebSocket发送目标
    Provider API key和Authorization header只存在于路由配置加载期、Adapter与ProviderHttpRequest私有字段
    TOML解析错误只返回行列位置和类别，不回显可能包含api_key的原文行
    AgentMessage、AgentFailure、InferenceError、日志和Tracing字段不得包含secret、完整请求正文或完整响应正文
    非2xx响应最多读取有界错误正文，提取常见错误字段并转换成ResponseStatus
    messages、工具Schema、单分片、累计响应和错误文本均设置大小上限
    ModelId路由不存在、失效Agent或缺少启动快照在主线程立即失败，不启动异步任务
    Provider执行与Adapter解析panic在推理任务内转换为可路由的TaskPanicked
    Runtime关闭导致的任务取消没有完成结果，只记录system log
```

## 持有关系

```text
App
└── World
    ├── GlobalModelRoutes Resource
    │   ├── path: PathBuf
    │   ├── factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>
    │   └── routes: HashMap<ModelId, ConfiguredModelRoute>
    ├── InferenceHttpClient Resource
    │   └── client: reqwest::Client
    ├── Workspace Entity
    │   └── WorkspaceModelRoutes
    │       └── routes: HashMap<ModelId, ConfiguredModelRoute>
    ├── AgentImage Entity
    │   └── AgentImageModelConfig--由AgentImageLoaderPlugin创建
    └── AgentInstance Entity
        └── AgentInferenceSnapshot
        │   ├── model: ModelId
        │   ├── parameters: InferenceParameters
        │   ├── workspace: Entity
            └── source_image: Entity

一次推理期间：
InferenceCommand
├── id: String
├── agent: Entity
├── agent_id: String
├── messages: Vec<Message>
└── tools: Vec<ToolDefinition>--由AgentPlugin遍历动态可见资源并逐个构造
    -> PreparedInference
       ├── route: InferenceRoute
       ├── client: reqwest::Client
       ├── request: ProviderHttpRequest
       ├── adapter: ErasedProviderAdapter
       ├── agent_id: String
       └── senders: Vec<WebSocketSender>
           -> InferenceTaskOutput
              ├── route: InferenceRoute
              └── result: Result<InferenceResponse, InferenceError>
                  ├── Ok -> AgentMessage
                  │   ├── id: String
                  │   ├── agent: Entity
                  │   ├── message: Message::Assistant
                  │   └── intent: CompleteTurn / DispatchToolCalls
                  └── Err -> AgentFailure
                      ├── id: String
                      ├── agent: Entity
                      ├── kind: Inference
                      └── message: String
```
