# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# 板块格式

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        图书馆组件与 Plugin
# system     src/system.rs     System 函数
# handler    src/handler.rs    处理函数
# events     src/events.rs     事件类型
# types      src/types.rs      其余类型
# error      src/error.rs      Error 与公开错误分类
```

# lib

lib 只放图书馆组件、Resource 和 Plugin。

InferencePlugin 不创建 Entity，不挂载任何领域 Component；AgentInferenceSnapshot 是 types 中定义的领域 Component，由 workspace_plugin 通过 WorldInferenceExt 挂载到 Agent。

## 类型

公开：
```text
GlobalModelRoutes：全局模型路由，公开Resource--由models.toml加载的默认Provider路由，Workspace未配置自己的models.toml时使用
    path: PathBuf--模型路由配置文件路径，私有
    factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>--已注册的api_type工厂，crate公开
    routes: HashMap<ModelId, ConfiguredModelRoute>--ModelId到模型路由的映射，私有
    load(path: PathBuf, factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>) -> Result<Self, InferenceError>
        加载全局路由：crate公开关联函数，读取并编译path处的models.toml
    reload(&mut self) -> Result<usize, InferenceError>
        重载全局路由：crate公开方法，重新读取同一路径并整体替换routes，返回路由数量
    get(&self, id: &ModelId) -> Option<ConfiguredModelRoute>
        读取路由：crate公开方法，按ModelId克隆返回配置
    impl Resource for GlobalModelRoutes

InferenceHttpClient：推理HTTP客户端，公开Resource--插件安装时创建的reqwest客户端
    client: reqwest::Client--共享HTTP客户端，crate公开
    new() -> Result<Self, InferenceError>
        创建客户端：crate公开关联函数
    impl Resource for InferenceHttpClient

InferencePlugin：推理插件，公开结构体--加载全局模型路由、创建HTTP客户端并挂载推理System
    schedule: String--System所属Schedule，私有
    config_path: PathBuf--全局models.toml路径，私有
    adapter_factories: HashMap<String, ErasedProviderAdapterFactory>--api_type到工厂的映射，私有
    new() -> Self
        构造插件：公开关联函数，默认PRE_UPDATE Schedule，注册openai和deepseek工厂
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开构建方法
    with_config_path(mut self, path: impl Into<PathBuf>) -> Self
        设置全局配置路径：公开构建方法
    with_api_type<Factory>(mut self, api_type: impl Into<String>, factory: Factory) -> Self
        注册API工厂：公开构建方法，api_type为空或已注册时panic
        约束：Factory: ProviderAdapterFactory
    impl Default for InferencePlugin
        Default：公开trait实现，调用new
    impl Plugin for InferencePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：要求RuntimePlugin和AsyncRuntimePlugin已安装
            行为：
                重复安装或Schedule不存在时panic
                加载GlobalModelRoutes并插入World
                创建InferenceHttpClient并插入World
                插入WorkspaceModelRoutesRegistry和InFlightInferences
                在schedule依次挂载reload_model_routes_system、cancel_inference_system、prepare_inference_system、execute_prepared_inference和publish_inference_output_system
```

## 逻辑

```text
InferencePlugin 不消费 AgentControl、不读取 AgentResourceMap、不注册 ToolSpec、不写历史或实时上下文。
全局模型路由由用户HOME下的.margatroid/models.toml提供；Workspace可以通过项目根目录.margatroid/models.toml覆盖。
InferencePlugin 的公开入口是事件（InferenceRequestEvent、ContextCompactionInferenceRequest、CapturedInferenceRequest、CancelInferenceRequest、ReloadModelRoutes）和WorldInferenceExt。
```

# system

system 放 System 函数。System 只读取本帧领域事件并克隆，然后调用 handler 中的对应处理函数；异步 System 直接消费 PreparedInference。

## 函数

crate公开：
```text
reload_model_routes_system(world: &mut World)
    重载路由System：crate公开System
    处理事件：ReloadModelRoutes
    行为：克隆本帧全部ReloadModelRoutes，逐个调用handle_reload_model_routes

prepare_inference_system(world: &mut World)
    准备推理System：crate公开System
    处理事件：InferenceRequestEvent、ContextCompactionInferenceRequest、CapturedInferenceRequest
    行为：
        读取三种请求并统一构造InferenceCommand
        普通推理输出到AgentMessage，压缩推理输出到ContextCompactionInferenceResponse，捕获推理输出到CapturedInferenceResponse
        逐个调用handle_prepare_inference

cancel_inference_system(world: &mut World)
    取消推理System：crate公开System
    处理事件：CancelInferenceRequest
    行为：克隆本帧全部CancelInferenceRequest，逐个调用handle_cancel_inference

execute_prepared_inference(prepared: PreparedInference, _context: AsyncContext) -> Result<InferenceTaskOutput, InferenceTaskError>
    执行推理：crate公开异步System
    行为：
        克隆路由和取消信号
        在取消信号与run_provider之间biased select
        取消时返回InferenceTaskResult::Cancelled
        run_provider panic时转换为TaskPanicked错误并返回Completed

publish_inference_output_system(world: &mut World)
    发布推理结果System：crate公开System
    处理事件：Result<InferenceTaskOutput, InferenceTaskError>
    行为：
        克隆本帧全部成功输出，Err输出仅记录warn日志
        对每个成功输出调用handle_inference_task_output
```

# handler

handler 放处理函数。每个 System 读到的领域事件在 handler 中展开为完整业务逻辑。

## 类型

公开：
```text
WorldInferenceExt：World推理扩展，公开trait--workspace_plugin和外部调用者配置模型路由与Agent推理快照
    reload_model_routes(&self, id: impl Into<String>)
        请求重载全局模型路由：公开方法，发送ReloadModelRoutes事件
    load_workspace_model_routes(&mut self, workspace: Entity, project_root: &Path) -> Result<usize, InferenceError>
        加载Workspace模型路由：公开方法
        行为：
            校验workspace存在
            project_root/.margatroid/models.toml不存在时移除Workspace路由并返回0
            读取GlobalModelRoutes中的工厂表，编译Workspace模型路由并写入WorkspaceModelRoutesRegistry
    build_agent_inference_snapshot(&self, workspace: Entity, source_image: Entity, config: &AgentImageModelConfig) -> Result<AgentInferenceSnapshot, InferenceError>
        构造Agent推理快照：公开方法
        行为：
            校验workspace和source_image存在
            由AgentImageModelConfig构造ModelId和InferenceParameters并校验
            确认模型可路由后读取全局或Workspace路由的context_window_tokens
            返回挂载到Agent的AgentInferenceSnapshot
    impl WorldInferenceExt for World
        行为：实现上述三个方法
```

crate公开：
```text
InFlightInferences：飞行中推理表，crate公开Resource
    requests: HashMap<(Entity, String), watch::Sender<bool>>--按Agent和turn_id定位取消信号，crate公开
    impl Resource for InFlightInferences

InferenceCommand：统一推理命令，crate公开结构体--三种推理事件在System中先转换为此结构
    id: String--交互轮次ID
    agent: Entity--目标Agent
    agent_id: ResourceId--稳定Agent身份
    messages: Vec<Message>--完整输入消息
    tools: Vec<ToolDefinition>--Provider无关ToolSpec
    output: InferenceOutputKind--输出路由
```

## 函数

crate公开：
```text
default_config_path() -> PathBuf
    默认配置路径：crate公开函数，返回HOME/.margatroid/models.toml

load_model_routes(path: &Path, factories: &HashMap<String, ErasedProviderAdapterFactory>) -> Result<HashMap<ModelId, ConfiguredModelRoute>, InferenceError>
    读取并编译模型路由：crate公开函数
    行为：读取TOML文件，限制最大字节数，调用compile_model_routes

summarize_reqwest_error(error: &reqwest::Error) -> String
    归纳HTTP错误：crate公开函数，按超时、连接、Body、解码、请求构造分类并拼接最深source

handle_reload_model_routes(world: &mut World, request: ReloadModelRoutes)
    处理路由重载：crate公开函数
    行为：校验ID非空后调用GlobalModelRoutes::reload并发送ReloadModelRoutesResult

handle_prepare_inference(world: &mut World, command: InferenceCommand)
    处理推理准备：crate公开函数
    行为：
        调用prepare_inference
        成功时发送PreparedInference异步事件
        失败时按route.output调用publish_inference_error

handle_cancel_inference(world: &mut World, cancellation: CancelInferenceRequest)
    处理取消推理：crate公开函数
    行为：在InFlightInferences中查找匹配请求并发送取消信号

run_provider(prepared: PreparedInference) -> Result<ProviderInferenceResponse, InferenceError>
    执行Provider请求：crate公开异步函数
    行为：
        发送HTTP请求并校验状态码
        失败时读取受限错误Body并解析Provider错误详情
        成功时按流式分片推送给前端并累积响应
        finish后发送尾部增量并返回ProviderInferenceResponse

handle_inference_task_output(world: &mut World, output: InferenceTaskOutput)
    处理推理任务输出：crate公开函数
    行为：
        从InFlightInferences移除当前请求；已取消或Cancelled直接返回
        AgentMessage输出：空正文且无工具调用视为ResponseIncomplete失败，否则发送携带usage的AgentMessage
        ContextCompaction输出：要求Completed、无tool_calls且正文非空，发送ContextCompactionInferenceResponse
        Captured输出：同上校验后发送margatroid_types::CapturedInferenceResponse
        失败时调用publish_inference_error

publish_inference_error(events: &RuntimeEventSender, route: InferenceRoute, error: InferenceError)
    发布推理错误：crate公开函数
    行为：
        AgentMessage输出发送AgentFailure { kind: Inference }
        ContextCompaction输出发送ContextCompactionInferenceResponse::Err
        Captured输出发送margatroid_types::CapturedInferenceResponse::Err
```

私有：
```text
parse_context_window(value: Option<&str>) -> Result<u64, InferenceError>
    解析上下文窗口：私有函数，支持k/m/b/t后缀，缺省1m，数量必须为无符号整数且大于0

compile_model_routes(source: &str, factories: &HashMap<String, ErasedProviderAdapterFactory>) -> Result<HashMap<ModelId, ConfiguredModelRoute>, InferenceError>
    编译模型路由：私有函数
    行为：
        解码TOML并要求models非空
        校验必填字段、ModelId唯一、base_url为http或https
        按api_type选择工厂构造Provider Adapter
        解析context_window并返回ModelId到ConfiguredModelRoute的映射

require_workspace(world: &World, workspace: Entity) -> Result<(), InferenceError>
    校验Workspace：私有函数，workspace不存在时返回AgentNotAlive

model_is_routable(world: &World, workspace: Entity, model: &ModelId) -> bool
    模型是否可路由：私有函数，先查Workspace路由再查全局路由

prepare_inference(world: &mut World, command: InferenceCommand) -> Result<PreparedInference, (InferenceRoute, InferenceError)>
    准备推理：私有函数
    行为：
        校验ID、agent_id类型、Agent存活、消息和工具
        读取Agent.info和AgentImage模型参数，确认模型可路由
        由Provider Adapter构造ProviderHttpRequest
        普通Agent推理解析WebSocket流式目标；压缩和捕获推理使用空发送器
        登记InFlightInferences并返回PreparedInference

resolve_websocket_targets(connections: &WebSocketConnections, targets: &[WebSocketMessageTarget]) -> Vec<WebSocketSender>
    解析流式目标：私有函数，按Broadcast、Type、Name去重收集发送器

validate_messages(messages: &[Message]) -> Result<(), InferenceError>
    校验消息：私有函数，限制单条和总大小，检查tool_call_id和assistant内容

validate_tools(tools: &[ToolDefinition]) -> Result<(), InferenceError>
    校验工具：私有函数，限制数量、名称字符、描述和Schema，拒绝重复名称

send_provider_request(client: &reqwest::Client, request: ProviderHttpRequest) -> Result<reqwest::Response, InferenceError>
    发送Provider请求：私有异步函数，失败时归纳reqwest错误

send_stream_delta(senders: &[WebSocketSender], id: &str, agent: &ResourceId, delta: ProviderStreamDelta) -> Result<(), InferenceError>
    发送流式增量：私有异步函数
    行为：reasoning发送agent.message.reasoning_delta，content发送agent.message.delta；忽略已关闭连接

safe_endpoint(url: &Url) -> String
    安全端点：私有函数，只保留origin的ascii序列化，去除凭据和路径

provider_error_detail(body: &[u8]) -> Option<String>
    Provider错误详情：私有函数
    行为：优先从JSON的error.message、error、message、detail提取；否则将文本控制字符替换为空格并压成单行

single_line(value: &str) -> String
    单行化：私有函数，控制字符替换为空格后按空白切分重新连接

read_bounded_body(response: reqwest::Response, limit: usize) -> Vec<u8>
    受限读取错误Body：私有异步函数，最多读取limit字节

context_compaction_content(response: &ProviderInferenceResponse) -> Result<String, InferenceError>
    压缩正文提取：私有函数，要求stop_reason=Completed、无tool_calls且content trim后非空
```

## 逻辑

```text
MclPlugin的inference Effect
    -> InferenceRequestEvent { messages, tools }
InferencePlugin主线程 prepare_inference_system
    -> InferenceCommand
    -> prepare_inference 构造 ProviderHttpRequest 和 WebSocket发送器
    -> PreparedInference 异步事件
异步任务 execute_prepared_inference
    -> run_provider 请求Provider
    -> reasoning_content 发送 agent.message.reasoning_delta
    -> content 发送 agent.message.delta
    -> ProviderInferenceResponse
主线程 publish_inference_output_system
    -> AgentMessage { Message::Assistant { reasoning, content, tool_calls }, usage }
AgentPlugin
    -> 把完整AgentMessage投递到长期Lua VM邮箱
Base Lua
    -> 下一次start取得Assistant消息并决定tool_call或finish

普通推理失败
    -> AgentFailure { id: turn_id, kind: Inference }
    -> MclPlugin完成当前或下一次start为Err并中止该turn，不留下永久pending回执

MclPlugin的catch_inference Effect
    -> CapturedInferenceRequest { messages, tools=[] }
    -> 不发送流式前端消息
    -> CapturedInferenceResponse
MclPlugin的context_compaction
    -> ContextCompactionInferenceRequest { messages }
    -> ContextCompactionInferenceResponse
```

# events

## 类型

公开：
```text
ReloadModelRoutes：全局模型路由重载请求，公开事件
    id: String--请求ID
    impl Event for ReloadModelRoutes

ReloadModelRoutesResult：全局模型路由重载结果，公开事件
    id: String--原请求ID
    result: Result<ModelRoutesReloaded, InferenceError>--成功时携带路由数量
    impl Event for ReloadModelRoutesResult

InferenceRequestEvent：普通推理请求事件，公开事件--由margatroid_types重新导出
    id: String--交互轮次ID
    agent: Entity--目标Agent
    agent_id: ResourceId--稳定Agent身份
    messages: Vec<Message>--完整上下文
    tools: Vec<ToolDefinition>--Provider无关ToolSpec
    impl Event for InferenceRequestEvent

ContextCompactionInferenceRequest：上下文压缩推理请求，公开事件--压缩Provider响应用到MCL上下文
    id: String--交互轮次ID
    agent: Entity--目标Agent
    agent_id: ResourceId--稳定Agent身份
    messages: Vec<Message>--待压缩消息
    impl Event for ContextCompactionInferenceRequest

ContextCompactionInferenceResponse：上下文压缩推理响应，公开事件
    id: String--原请求ID
    agent: Entity--目标Agent
    result: Result<String, InferenceError>--成功时为非空压缩正文
    impl Event for ContextCompactionInferenceResponse

CapturedInferenceRequest：捕获式推理请求，公开事件--由margatroid_types重新导出
    id: String--"mcl-effect:"命名空间下的请求ID
    agent: Entity--目标Agent
    agent_id: ResourceId--稳定Agent身份
    messages: Vec<Message>--MCL RefBlock展开后的完整输入
    impl Event for CapturedInferenceRequest

CapturedInferenceResponse：捕获式推理响应，公开事件--由margatroid_types重新导出
    id: String--原捕获请求ID
    agent: Entity--目标Agent
    result: Result<String, InferenceError>--成功时为非空完整正文
    impl Event for CapturedInferenceResponse

CancelInferenceRequest：取消推理请求，公开事件
    id: String--要取消的交互轮次ID
    agent: Entity--目标Agent
    impl Event for CancelInferenceRequest
```

crate公开：
```text
InferenceRoute：推理路由，crate公开结构体
    id: String--交互轮次ID
    agent: Entity--目标Agent
    output: InferenceOutputKind--输出类型

InferenceOutputKind：推理输出类型，crate公开枚举
    AgentMessage
    ContextCompaction
    Captured

PreparedInference：已准备推理，crate公开事件--主线程完成路由、Provider适配和发送器解析后的异步任务输入
    route: InferenceRoute--推理路由
    agent_id: ResourceId--稳定Agent身份
    client: reqwest::Client--共享HTTP客户端
    request: ProviderHttpRequest--已编码Provider请求
    adapter: ErasedProviderAdapter--Provider协议适配器
    senders: Vec<WebSocketSender>--普通Agent推理的流式目标；压缩和捕获为空
    cancellation: watch::Receiver<bool>--取消信号
    impl Event for PreparedInference

InferenceTaskOutput：异步推理结果，crate公开结构体--publish_inference_output_system读取的任务输出
    route: InferenceRoute--推理路由
    result: InferenceTaskResult--推理结局

InferenceTaskResult：异步推理结局，crate公开枚举
    Completed(Result<ProviderInferenceResponse, InferenceError>)
    Cancelled

InferenceTaskError：异步任务错误，crate公开结构体--携带AsyncTaskError供日志
    source: AsyncTaskError--异步运行时错误
    impl From<AsyncTaskError> for InferenceTaskError
```

# types

types 放除事件和错误外的其余类型、Provider 协议适配器和响应累积器。

## 常量

crate公开：
```text
MAX_CONFIG_BYTES: usize = 1024 * 1024
MAX_MESSAGES_BYTES: usize = 16 * 1024 * 1024
MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024
MAX_TOOL_DESCRIPTION_BYTES: usize = 8 * 1024
MAX_TOOL_COUNT: usize = 256
MAX_STOP_COUNT: usize = 64
MAX_STOP_BYTES: usize = 256
MAX_ERROR_BODY_BYTES: usize = 16 * 1024
```

## 类型

公开：
```text
ModelId：模型ID，公开结构体--非空、不超过256字符且不含控制字符
    new(value: impl Into<String>) -> Result<Self, InferenceError>
        构造模型ID：公开关联函数
    as_str(&self) -> &str
        读取模型ID：公开方法
    impl fmt::Display for ModelId
        Display：公开trait实现，直接输出内部字符串

InferenceParameters：推理参数，公开结构体--Provider无关采样参数
    temperature: Option<f32>--温度，私有
    max_output_tokens: Option<u32>--最大输出Token，私有
    top_p: Option<f32>--核采样，私有
    stop: Vec<String>--停止序列，私有
    new(temperature: Option<f32>, max_output_tokens: Option<u32>, top_p: Option<f32>, stop: Vec<String>) -> Self
        构造参数：公开关联函数
    temperature(&self) -> Option<f32>
        读取温度：公开方法
    max_output_tokens(&self) -> Option<u32>
        读取最大输出Token：公开方法
    top_p(&self) -> Option<f32>
        读取核采样：公开方法
    stop(&self) -> &[String]
        读取停止序列：公开方法
    validate(&self) -> Result<(), InferenceError>
        校验参数：crate公开方法，校验范围、停止序列数量和长度

AgentInferenceSnapshot：Agent固定推理配置，公开Component--workspace_plugin通过WorldInferenceExt挂载
    model: ModelId--模型ID，crate公开
    context_window_tokens: u64--模型上下文窗口Token数，crate公开
    parameters: InferenceParameters--采样参数，crate公开
    workspace: Entity--所属Workspace，crate公开
    source_image: Entity--来源AgentImage，crate公开
    model(&self) -> &ModelId
        读取模型ID：公开方法
    context_window_tokens(&self) -> u64
        读取上下文窗口：公开方法
    parameters(&self) -> &InferenceParameters
        读取参数：公开方法
    workspace(&self) -> Entity
        读取Workspace：公开方法
    source_image(&self) -> Entity
        读取来源镜像：公开方法
    impl Component for AgentInferenceSnapshot

ConfiguredModelRoute：模型路由配置，公开结构体
    model: String--Provider模型名，crate公开
    context_window_tokens: u64--上下文窗口Token数，crate公开
    adapter: ErasedProviderAdapter--协议适配器，crate公开
    model(&self) -> &str
        读取Provider模型名：公开方法
    context_window_tokens(&self) -> u64
        读取上下文窗口：公开方法
    adapter(&self) -> &ErasedProviderAdapter
        读取适配器：公开方法

WorkspaceModelRoutes：Workspace模型路由，公开结构体
    routes: HashMap<ModelId, ConfiguredModelRoute>--ModelId到模型路由，crate公开
    get(&self, id: &ModelId) -> Option<ConfiguredModelRoute>
        读取路由：公开方法
    len(&self) -> usize
        路由数量：公开方法
    is_empty(&self) -> bool
        是否为空：公开方法

WorkspaceModelRoutesRegistry：Workspace模型路由注册表，公开Resource
    routes: HashMap<Entity, WorkspaceModelRoutes>--Workspace到路由，私有
    get(&self, workspace: Entity) -> Option<&WorkspaceModelRoutes>
        读取Workspace路由：公开方法
    insert(&mut self, workspace: Entity, routes: WorkspaceModelRoutes)
        写入Workspace路由：公开方法
    remove(&mut self, workspace: Entity)
        移除Workspace路由：公开方法
    impl Resource for WorkspaceModelRoutesRegistry

ProviderInput<'a>：Provider无关请求输入，公开结构体
    model: &'a str--Provider模型名，私有
    parameters: &'a InferenceParameters--采样参数，私有
    messages: &'a [Message]--消息，私有
    tools: &'a [ToolDefinition]--内部ToolSpec，私有
    new(model: &'a str, parameters: &'a InferenceParameters, messages: &'a [Message], tools: &'a [ToolDefinition]) -> Self
        构造输入：crate公开关联函数
    model(&self) -> &str
        读取模型名：公开方法
    parameters(&self) -> &InferenceParameters
        读取参数：公开方法
    messages(&self) -> &[Message]
        读取消息：公开方法
    tools(&self) -> &[ToolDefinition]
        读取工具：公开方法

ProviderRouteInput<'a>：Provider路由输入，公开结构体--从ModelRouteConfig构造后交给工厂
    provider: Option<&'a str>--Provider覆盖名，crate公开
    base_url: &'a Url--基础URL，crate公开
    api_key: &'a str--API Key，crate公开
    thinking: Option<&'a str>--DeepSeek思考开关，crate公开
    reasoning_effort: Option<&'a str>--DeepSeek思考强度，crate公开
    provider(&self) -> Option<&str>
        读取Provider覆盖名：公开方法
    base_url(&self) -> &Url
        读取基础URL：公开方法
    api_key(&self) -> &str
        读取API Key：公开方法
    thinking(&self) -> Option<&str>
        读取思考开关：公开方法
    reasoning_effort(&self) -> Option<&str>
        读取思考强度：公开方法

ProviderHttpRequest：已编码Provider请求，公开结构体
    method: Method--HTTP方法，crate公开
    url: Url--请求URL，crate公开
    headers: HeaderMap--请求头，crate公开
    body: Vec<u8>--请求体，crate公开
    new(method: Method, url: Url, headers: HeaderMap, body: Vec<u8>) -> Self
        构造请求：公开关联函数

ProviderAdapter：Provider协议适配器，公开trait
    继承：Send + Sync + 'static
    build_request(&self, input: ProviderInput<'_>) -> Result<ProviderHttpRequest, InferenceError>
        构造Provider请求：把Provider无关输入编码为HTTP请求
    begin_response(&self, status: StatusCode, headers: &HeaderMap) -> Result<Box<dyn ProviderResponseAccumulator>, InferenceError>
        开始响应：校验状态码并创建对应协议累积器

ProviderAdapterFactory：Provider适配器工厂，公开trait
    继承：Send + Sync + 'static
    build(&self, route: ProviderRouteInput<'_>) -> Result<ErasedProviderAdapter, InferenceError>
        构造适配器：校验路由选项并返回适配器对象

ProviderResponseAccumulator：Provider响应累积器，公开trait
    继承：Send + 'static
    push(&mut self, chunk: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError>
        累积流式分片：解析SSE并返回有序增量
    finish(self: Box<Self>) -> Result<(ProviderInferenceResponse, Vec<ProviderStreamDelta>), InferenceError>
        完成响应：返回最终响应和尾部增量

ErasedProviderAdapter：Provider适配器对象，公开类型别名
    Arc<dyn ProviderAdapter>

ErasedProviderAdapterFactory：Provider适配器工厂对象，公开类型别名
    Arc<dyn ProviderAdapterFactory>

StopReason：停止原因，公开枚举
    Completed
    ToolCalls
    Length
    ContentFilter
    Other(String)

ProviderInferenceResponse：Provider响应，公开结构体--协议无关累积结果
    reasoning: Option<String>--Provider公开的完整思考内容
    content: Option<String>--Assistant文本
    tool_calls: Vec<ToolCall>--保留Provider返回的调用ID、tool_name和参数
    stop_reason: StopReason--停止原因
    usage: Option<TokenUsage>--Provider返回usage时保存输入、输出和缓存命中Token

ProviderStreamDelta：Provider流式增量，公开枚举--累积器区分思考与正文的有序输出
    Reasoning(String)--新增思考文本
    Content(String)--新增正文文本

ModelRoutesReloaded：模型路由重载结果，公开结构体
    route_count: usize--重载后的路由数量

OpenAiAdapterFactory：OpenAI协议工厂，公开结构体
    new() -> Self
        构造工厂：公开关联函数
    impl ProviderAdapterFactory for OpenAiAdapterFactory
        构造OpenAI适配器：拒绝DeepSeek思考选项，校验API Key不含控制字符

DeepSeekAdapterFactory：DeepSeek协议工厂，公开结构体
    new() -> Self
        构造工厂：公开关联函数
    impl ProviderAdapterFactory for DeepSeekAdapterFactory
        构造DeepSeek适配器：校验thinking和reasoning_effort组合，启用思考时要求high或max
```

crate公开：
```text
ModelRouteDocument：models.toml文档，crate公开结构体
    models: Vec<ModelRouteConfig>--模型路由列表

ModelRouteConfig：模型路由配置，crate公开结构体
    id: String--模型ID
    model: String--Provider模型名
    provider: Option<String>--Provider覆盖名
    base_url: String--基础URL
    api_key: String--API Key
    api_type: String--选择Provider Adapter
    thinking: Option<String>--DeepSeek思考开关
    reasoning_effort: Option<String>--DeepSeek思考强度
    context_window: Option<String>--模型总上下文窗口，使用k/m/b/t后缀
```

私有：
```text
OpenAiAdapter：OpenAI协议适配器，私有结构体
    base_url: Url
    api_key: String
    impl ProviderAdapter for OpenAiAdapter
        build_request：使用/chat/completions和流式SSE，构造OpenAI兼容请求体
        begin_response：非成功状态返回ResponseStatus

DeepSeekAdapter：DeepSeek协议适配器，私有结构体
    base_url: Url
    api_key: String
    thinking: bool
    reasoning_effort: Option<String>
    impl ProviderAdapter for DeepSeekAdapter
        build_request：使用/chat/completions和流式SSE，写入thinking与reasoning_effort
        begin_response：非成功状态返回ResponseStatus

OpenAiRequest：OpenAI兼容请求体，私有结构体
    model: String
    messages: Vec<serde_json::Value>
    tools: Vec<serde_json::Value>
    stream: bool
    stream_options: OpenAiStreamOptions
    temperature: Option<f32>
    max_tokens: Option<u32>
    top_p: Option<f32>
    stop: Vec<String>
    thinking: Option<DeepSeekThinking>
    reasoning_effort: Option<String>
    from_input(input: ProviderInput<'_>) -> Self
        从通用输入构造OpenAI请求：私有关联函数
    from_deepseek_input(input: ProviderInput<'_>, thinking: bool, reasoning_effort: Option<String>) -> Self
        从通用输入构造DeepSeek请求：私有关联函数

OpenAiStreamOptions：流选项，私有结构体
    include_usage: bool

DeepSeekThinking：DeepSeek思考开关，私有结构体
    thinking_type: &'static str

OpenAiAccumulator：OpenAI兼容SSE累积器，私有结构体
    buffer: Vec<u8>
    reasoning: String
    content: String
    tool_calls: Vec<OpenAiToolCallBuilder>
    stop_reason: Option<StopReason>
    usage: Option<TokenUsage>
    saw_choice: bool
    done: bool
    capture_reasoning: bool
    impl ProviderResponseAccumulator for OpenAiAccumulator
        push：按行解析SSE，返回本帧产生的增量
        finish：处理尾部行，校验完整性并返回ProviderInferenceResponse
    consume_line(&mut self, line: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError>
        消费单行SSE：私有方法
    consume_chunk(&mut self, chunk: OpenAiChunk) -> Result<Vec<ProviderStreamDelta>, InferenceError>
        消费单个JSON帧：私有方法，累积usage、content、reasoning和tool_calls

OpenAiToolCallBuilder：OpenAI工具调用累积，私有结构体
    id: String
    name: String
    arguments: String

OpenAiChunk：OpenAI流式JSON帧，私有结构体
OpenAiChoice：OpenAI选择，私有结构体
OpenAiDelta：OpenAI增量，私有结构体
OpenAiToolCallDelta：OpenAI工具调用增量，私有结构体
OpenAiFunctionDelta：OpenAI函数增量，私有结构体
OpenAiUsage：OpenAI用量，私有结构体
OpenAiTokenDetails：OpenAI Token详情，私有结构体

DeepSeekAccumulator：DeepSeek SSE累积器，私有结构体
    inner: OpenAiAccumulator
    impl ProviderResponseAccumulator for DeepSeekAccumulator
        push：开启reasoning捕获后委托OpenAiAccumulator
        finish：开启reasoning捕获后委托OpenAiAccumulator
```

## 函数

私有：
```text
openai_message(message: &Message) -> serde_json::Value
    OpenAI消息转换：私有函数，System、User、Assistant、Tool转OpenAI JSON；Assistant无tool_calls时不写tool_calls

deepseek_message(message: &Message) -> serde_json::Value
    DeepSeek消息转换：私有函数，只有带tool_calls的Assistant写reasoning_content；其余委托openai_message

decode_token_usage(usage: OpenAiUsage) -> TokenUsage
    统一Token统计：私有函数
    行为：输入与输出兼容input_tokens/output_tokens和prompt_tokens/completion_tokens；缓存命中依次兼容prompt_tokens_details.cached_tokens、input_tokens_details.cached_tokens和prompt_cache_hit_tokens

parse_stop_reason(value: &str) -> StopReason
    解析停止原因：私有函数，stop映射Completed，tool_calls/function_call映射ToolCalls，length映射Length，content_filter映射ContentFilter
```

## 逻辑

```text
Provider Adapter 构造：
    compile_model_routes 读取 api_type 并选择工厂
    OpenAiAdapterFactory/DeepSeekAdapterFactory 校验路由选项
    OpenAiAdapter/DeepSeekAdapter 在 build_request 中把内部ToolSpec转换为Provider ToolSpec
    resource_name 原样作为本次请求的 Provider tool_name
    stream_options.include_usage = true 以请求末尾Token统计

Provider 响应累积：
    OpenAiAccumulator 解析SSE行，按顺序产生 Content 与 Reasoning 增量
    DeepSeekAccumulator 开启 reasoning_content/reasoning 捕获后委托 OpenAiAccumulator
    finish 时使用 decode_token_usage 统一Token统计，并把工具调用还原为内部 ToolCall

边界：
    types 不查询 AgentResourceMap，不写 AgentResourceMap。
    api_type 决定协议语义，base_url 只决定请求目的地。
```

# error

error 放 Error 类型和公开错误分类。

## 类型

公开：
```text
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

InferenceError：推理错误，公开结构体
    kind: InferenceErrorKind--错误分类，私有
    message: String--有界错误消息，私有
    status: Option<u16>--HTTP状态码，私有
    new(kind: InferenceErrorKind, message: impl Into<String>) -> Self
        构造错误：公开关联函数
    with_status(kind: InferenceErrorKind, status: Option<u16>, message: impl Into<String>) -> Self
        构造带状态错误：公开关联函数，消息超过512字节时截断为509字节加"..."
    kind(&self) -> InferenceErrorKind
        读取分类：公开方法
    message(&self) -> &str
        读取消息：公开方法
    status(&self) -> Option<u16>
        读取HTTP状态：公开方法
    panic(self) -> !
        panic：crate公开方法，直接panic该错误，只用于Plugin安装阶段
    impl fmt::Display for InferenceError
        Display：公开trait实现，有status时输出"{kind:?} (HTTP {status}): {message}"
    impl std::error::Error for InferenceError
```

## 逻辑

```text
InferenceError 只用于推理域错误。AgentFailure 由 handler 中的 publish_inference_error 使用 AgentFailureKind::Inference 包装，不经过 InferenceErrorKind 之外的公开分类。
InferenceError 消息会被截断到512字节以内，避免Provider错误详情无限增长。
```
