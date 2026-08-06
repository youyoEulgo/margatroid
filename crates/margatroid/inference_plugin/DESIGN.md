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
    validate(&self) -> Result<(), InferenceError>
        验证参数：crate公开方法，检查数值范围、停止序列数量与长度
    impl Default for InferenceParameters
        Default：公开trait实现，全部可选参数为空且stop为空

AgentInferenceSnapshot：Agent实例推理快照，公开组件--启动时从中立AgentImageModelConfig转换，不随AgentImage编辑自动更新
    model: ModelId--实际使用的逻辑模型ID
    parameters: InferenceParameters--实际使用的参数
    workspace: Entity--所属Workspace Entity，用于查询项目级模型路由
    source_image: Entity--来源AgentImage Entity，仅用于追踪，不在推理时重新读取
    impl Clone for AgentInferenceSnapshot
        Clone：公开trait实现，允许Workspace为Agent实例准备独立推理配置
    impl Component for AgentInferenceSnapshot
        Component：公开trait实现

WorkspaceModelRoutes：Workspace模型路由，公开组件--挂在Workspace Entity上的项目级路由覆盖
    routes: HashMap<ModelId, ConfiguredModelRoute>--从<project>/.margatroid/models.toml编译的项目级路由，私有
    get(&self, id: &ModelId) -> Option<ConfiguredModelRoute>
        取得项目路由：crate公开方法，克隆实际模型名称和Adapter共享引用
    impl Component for WorkspaceModelRoutes
        Component：公开trait实现

InferenceCommand：推理命令，公开事件--携带已处理的messages、AgentPlugin收集的工具定义与可选前端文本流
    id: String--调用方生成的请求ID，与agent共同构成并发响应路由
    agent: Entity--发起本次推理的AgentInstance Entity
    messages: Vec<Message>--本次请求的完整消息快照
    tools: Vec<ToolDefinition>--AgentPlugin遍历动态可见资源并逐个构造后收集的工具定义
    stream: Option<InferenceStreamSender>--可选有界文本通道发送器，用于直接转发给前端
    impl Event for InferenceCommand
        Event：公开trait实现

InferenceStreamSender：推理文本流发送器，公开类型别名--等于tokio::sync::mpsc::Sender<String>

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
    ResponseIncomplete
    TaskPanicked

InferenceError：推理错误，公开结构体--不保存API key、Authorization header、完整请求或完整响应
    kind: InferenceErrorKind--错误分类
    message: String--有界诊断文本
    status: Option<u16>--Provider HTTP状态码
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

ProviderRouteInput<'a>：Provider路由视图，公开结构体--ProviderAdapterFactory创建已配置Adapter时只读借用
    provider: Option<&'a str>--可选供应商名称，不参与路由查找
    base_url: &'a reqwest::Url--完整API基础地址
    api_key: &'a str--鉴权密钥，不得保存到日志、Event或Error

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
    finish(self: Box<Self>) -> Result<InferenceResponse, InferenceError>
        完成响应：公开方法，验证协议结束、工具调用完整性并返回统一响应

InferencePlugin：推理插件，公开结构体
    schedule: String--命令准备、异步提交与结果发布所属Schedule，私有
    config_path: PathBuf--主目录全局模型路由表路径，私有
    adapter_factories: HashMap<String, ErasedProviderAdapterFactory>--api_type到协议工厂的映射，私有
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
    stream: Option<InferenceStreamSender>--可选前端文本流发送器
    impl Event for PreparedInference
        Event：私有trait实现

InferenceTaskOutput：异步推理任务输出，私有结构体--无论成功失败都保留原路由
    route: InferenceRoute--响应路由
    result: Result<InferenceResponse, InferenceError>--Provider执行结果

InferenceTaskError：异步监督错误，私有结构体--表示AsyncRuntime在推理处理器外层取消任务
    source: AsyncTaskError--异步运行时错误
    impl From<AsyncTaskError> for InferenceTaskError
        From<AsyncTaskError>：私有trait实现，满足AppAsyncExt::add_async_system的错误约束
```

## 函数

私有：
```text
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

execute_prepared_inference(prepared: PreparedInference) -> Result<InferenceTaskOutput, InferenceTaskError>
    执行推理：私有异步函数，发送HTTP请求、向前端通道转发文本并累积完整响应
    行为：
        始终保留prepared.route
        在panic捕获边界内使用prepared.client发送prepared.request
        调用Adapter::begin_response创建独占累积器
        异步读取response.bytes_stream
        每取得一段bytes就调用累积器::push
        prepared.stream存在时将push返回的文本片段通过有界通道发送
        前端接收端已关闭时丢弃后续文本转发，不中止后端推理与累积
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
    行为：设置method、url、headers和body；网络错误转换为RequestFailed；不记录header和body
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
        -> 无前端实时输出时send_event(InferenceCommand { id, agent, messages, tools, stream: None })
        -> 需要前端实时输出时创建有界文本通道并传入stream: Some(sender)
    prepare_inference_system
        -> 读取AgentInferenceSnapshot并使用command.tools
        -> 根据snapshot.workspace查询WorkspaceModelRoutes
        -> 项目级未找到snapshot.model时查询全局GlobalModelRoutes
        -> Adapter把统一Message、工具和实际模型配置组装为ProviderHttpRequest
        -> 克隆InferenceHttpClient内的reqwest::Client
        -> send_async_event(PreparedInference)
    异步System取得PreparedInference
        -> 发送HTTP请求
        -> 按网络分片持续喂给ProviderResponseAccumulator
        -> 有stream时将可展示文本片段写入有界通道
        -> 工具调用片段只在累积器内部组装
        -> 响应结束后返回InferenceTaskOutput
    publish_inference_output_system
        -> 成功时发送AgentMessage { id, agent, Message::Assistant, intent }
        -> 失败时发送AgentFailure { id, agent, kind: Inference, message }

流式响应：
    文本片段只用于前端实时展示，通过InferenceStreamSender发送
    文本片段不进入ECS事件队列，不直接写入Agent messages
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
    Provider API key和Authorization header只存在于路由配置加载期、Adapter与ProviderHttpRequest私有字段
    TOML解析错误只返回行列位置和类别，不回显可能包含api_key的原文行
    AgentMessage、AgentFailure、InferenceError、日志和Tracing字段不得包含secret、完整请求正文或完整响应正文
    非2xx响应最多读取有界错误正文，由Adapter转换成ResponseStatus
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
├── messages: Vec<Message>
├── tools: Vec<ToolDefinition>--由AgentPlugin遍历动态可见资源并逐个构造
└── stream: Option<InferenceStreamSender>
    -> PreparedInference
       ├── route: InferenceRoute
       ├── client: reqwest::Client
       ├── request: ProviderHttpRequest
       ├── adapter: ErasedProviderAdapter
       └── stream: Option<InferenceStreamSender>
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
