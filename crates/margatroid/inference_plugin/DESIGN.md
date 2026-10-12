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

```text
lib        插件、全局配置Resource与客户端快照边界
system     src/system.rs             请求及配置重载调度
handler    src/handler.rs            主线程准备与异步执行
events     共享types/inference_events.rs    全部请求、回复载荷与监督关联
types      src/types.rs及共享纯值契约       Provider协议与配置辅助类型
error      共享推理错误契约               校验、执行及协议错误
```

**本文为重新设计的目标方案，尚未实现，不是当前源码参考。** 一个推理入口、一个准备System、一项异步闭包、一个oneshot结果。全局只有一份模型配置；提交前取得全部已注册webui客户端发送器快照，闭包直接推流，后来连接不加入本次推理。

# lib

## 类型

公开：
```text
InferencePlugin：推理插件，公开结构体
    schedule: String--推理准备及重载计划
    config_path: PathBuf--全局models.toml路径
    factories: HashMap<String, ErasedProviderAdapterFactory>--协议工厂
    new() -> Self
        构造插件：公开关联函数
        行为：默认PRE_UPDATE计划、HOME/.margatroid/models.toml，注册openai与deepseek工厂。
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法
        行为：保存计划名并返回自身。
    with_config_path(mut self, path: impl Into<PathBuf>) -> Self
        设置配置路径：公开方法
        行为：保存全局路径，不新增工作区配置来源。
    with_api_type<F>(mut self, api_type: impl Into<String>, factory: F) -> Self
        注册协议工厂：公开方法
        约束：F: ProviderAdapterFactory
        行为：名称为空或重复则拒绝，保存工厂并返回自身。
    impl Default for InferencePlugin
        default() -> Self
            默认构造：公开trait方法
            行为：调用new。
    impl Plugin for InferencePlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                检查计划、通用闭包执行设施及公共事件能力，重复安装拒绝
                有界读取并完整编译全局模型配置，失败拒绝安装
                创建共享HTTP客户端，插入InferenceConfig和安装标记
                只挂载inference_system，不安装固定异步System或结果发布System

InferenceConfig：推理配置，公开Resource--实现侧配置，不是图书馆组件
    path: PathBuf--唯一模型配置路径
    routes: HashMap<ModelId, ConfiguredModelRoute>--全局路由
    factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>--重载时使用的工厂
    client: reqwest::Client--共享连接池
    limits: InferenceLimits--输入、响应、超时及推流上限，私有字段
    reload_id: Option<String>--当前全局配置重载关联，私有字段；初始None
    impl Resource for InferenceConfig
```

## 逻辑

```text
领域职责：
    不创建Entity、不挂图书馆组件，不保存Agent上下文、turn或工具执行状态
    Agent.info.model提供模型选择与采样参数快照，不在每次推理时重新读取AgentImage
    全局只有InferenceConfig这一份配置，不设WorkspaceModelRoutes或路由注册表
    Inference只执行一次模型请求，自动历史、MCL队列及指令结果由MCL负责
    不接收AgentControl，不投递Lua邮箱，不发AgentFailure代替oneshot错误

客户端边界：
    主线程遍历已注册Client，筛选client_type为webui的连接，通过其连接句柄取得发送器
    发送器去重并复制到闭包，未注册连接不纳入快照；无发送器也正常推理
    使用共享Client组件与连接查询契约，不依赖ClientPlugin Handler
    通过共享WorldClientExt.client_webui_senders()取得已注册webui发送器快照，不依赖客户端实现Handler
    推流不按Workspace隔离；推流关联包含不透明agent_instance字符串及Agent逻辑地址供客户端区分；agent_instance仅为关联值，不能作为协议请求目标或解码为Entity
```

# system

## 函数

crate公开：
```text
inference_system(world: &mut World)
    推理System：crate公开System
    处理事件：InferenceRequest、ReloadModelRoutes、Result<InferenceReloadOutput, InferenceReloadTaskError>
    产出事件：ClosureRequest、ReloadModelRoutesResult
    行为：
        先处理本帧重载闭包结果，逐个调用handle_model_routes_reloaded
        再克隆重载请求并调用handle_reload_model_routes
        最后克隆推理请求并调用handle_inference，克隆只共享同一回复槽
        无请求不轮询Agent或推理任务，没有业务推理结果转发阶段
```

## 逻辑

```text
推理完成路径：
    异步执行直接发送oneshot，结果无需回写World
    通用闭包Result只承担监督，不另建InferenceResult转发或权威结果通道
    重载需要替换World中的配置，所以其异步结果由同一个System收口
    重载期间新推理继续使用旧配置，不阻塞ECS主线程
```

# handler

## 函数

crate公开：
```text
handle_inference(world: &mut World, request: InferenceRequest)
    准备推理：crate公开函数
    行为：
        校验请求ID、完整Agent Entity及取消状态；已失效或取消直接完成oneshot错误
        复制Agent地址、info.model中的模型与采样参数，从全局配置查唯一模型路由
        收集本次可见资源的工具定义，规范化消息，验证参数与输入上限
        收集全部当前webui发送器快照，复制HTTP客户端、路由及协议适配器
        保存拥有所有权的PreparedInference，不携带World、组件借用或Workspace配置
        通过Agent.execution共享AgentExecutionScope受检入口登记Inference活动，令牌从该作用域派生；登记与沙箱更新复用同一互斥门，但Inference活动不阻止沙箱改变
        建立一次性回复保障，提交执行闭包；准备或提交失败均完成原回复槽，提交失败撤销未启动活动登记
        不要求Agent.turn非空，不用turn代号作为推理请求唯一ID

handle_reload_model_routes(world: &mut World, request: ReloadModelRoutes)
    发起全局配置重载：crate公开函数
    行为：
        校验ID，若本次重载仍在途则返回ConfigBusy，不并行发布不同版本
        复制唯一配置路径和工厂，记录重载请求关联，异步有界读取并编译完整配置
        返回InferenceReloadOutput，读取、解析或监督失败都保留旧配置
        只重载全局配置，不接受workspace或项目路径覆盖

handle_model_routes_reloaded(world: &mut World, output: Result<InferenceReloadOutput, InferenceReloadTaskError>)
    收口配置重载：crate公开函数
    行为：
        核对本次重载关联，重复或迟到结果不替换其他版本
        成功时整体替换routes，失败保持原配置；清除本次重载标记
        发送关联原请求的ReloadModelRoutesResult，已开始推理继续使用自身路由快照
```

私有：
```text
collect_tool_definitions(world: &World, agent: Entity, visible: &[ResourceID]) -> Result<Vec<ToolDefinition>, InferenceError>
    准备可见工具定义：私有函数
    行为：
        对visible完整ResourceID去重，只读取目标Agent Entity的资源集合
        仅遍历tool与skill可调用资源条目，筛选map key属于visible的项，get_alias作为模型工具名
        tool复制description和已加载输入schema；skill复制description并生成消费侧固定schema {"type":"object","properties":{},"additionalProperties":false}（参数仅{}），与ToolPlugin校验一致，不要求skill包增加schema文件；不读取或执行Lua脚本
        指定ID缺失或缺乏模型工具定义时返回错误，不悄悄忽略；visible指定hook、prompt或sandbox等非可调用类型时返回UnsupportedResource
        重复alias、非法工具名或不合法schema拒绝，空visible表示无工具，不回退默认集合
        可调用集合与ToolPlugin保持tool+skill一致；hook仅为Driver解释的触发器，即使有schema也不暴露为模型工具，不把prompt/sandbox冒充工具

normalize_messages(messages: Vec<AgentMessage>) -> Result<Vec<ProviderMessage>, InferenceError>
    规范化输入消息：私有函数
    行为：
        inject递归按顺序展开内部AgentMessage数组，限制嵌套深度及展开后的数量和字节数
        user、assistant、tool_call转为对应Provider消息；error转用户错误文本
        保留assistant reasoning、工具调用ID及参数，工具结果保留对应调用ID
        历史usage不作为Provider请求内容；未知或无法表达的结构返回错误，不静默丢弃
        检查工具调用与结果的关联及必要字段，空输入及超限拒绝

collect_webui_senders(world: &World) -> Vec<WebSocketSender>
    收集当前推流发送器：私有函数
    行为：调用WorldClientExt.client_webui_senders()，去重，忽略已断开的连接；返回本次快照，不保存全局推理订阅表。

run_inference(prepared: PreparedInference) -> Result<InferenceResponse, InferenceError>
    执行模型请求：私有异步函数
    行为：
        在取消令牌、回复Receiver关闭与整体超时之间监督执行，不无限等待网络
        使用Adapter构造请求并发送HTTP，校验状态；失败有界读取错误body
        成功时创建本次ResponseAccumulator，逐分片解析并累积assistant、工具调用和usage
        解析到正文或reasoning增量就直接推送快照中的发送器，不发领域增量事件再中转
        finish必须确认协议完整结束并推送尾部增量；网络提前结束、非法分片或超限返回错误
        完整工具调用即使没有正文也可成功；Length返回已有结果及截断原因，不自动续写
        ContentFilter保留真实停止原因；不把残缺网络响应当作正常截断
        生成带既有内容及usage的assistant AgentMessage和stop_reason，返回完整结果
        不查询Agent，不写历史，不执行工具，不重试请求

execute_inference(prepared: PreparedInference)
    执行并完成回复：私有异步函数
    行为：
        运行run_inference，捕获执行失败或panic并转换为InferenceError
        在短锁内take回复槽中的唯一Sender，发送一次Result；重复完成不再发送
        取消、超时、提前返回及任务丢弃由私有回复保障完成关联错误或关闭Sender，不能只记录日志
        等待HTTP请求、流读取及本次推流执行真实结束并释放资源后，才mark_completed并释放Inference活动登记
        正常运行结束后关闭保障，不再发送第二次错误；外层框架监督失败也不能留下永久等待，cleanup未完成不得仅靠Drop或关闭Sender标记活动完成

push_stream(senders: &mut Vec<WebSocketSender>, id: &str, agent_instance: &str, agent_id: &ResourceID, sequence: u64, delta: ProviderStreamDelta)
    直接推送增量：私有异步函数
    行为：
        保留正文与reasoning不同消息种类，携带请求ID、Agent地址、不透明agent_instance及递增序号；内部完整Entity只用于实例核对，不作为推流DTO字段或外部请求目标
        使用客户端现有消息编码契约，发送器快照从开始到结束不再补充后来连接
        关闭发送器从本次活动列表剔除；客户端失败不改变模型结果
        不因慢客户端无限阻塞模型读取，使用有界队列或发送超时并停止向该慢连接推送
        不逐增量写历史，不把未完整的工具参数当作可执行调用
        最终成功或失败可向快照发送器发送终态标记，oneshot仍是唯一权威完成结果

load_model_routes(path: &Path, factories: &HashMap<String, ErasedProviderAdapterFactory>) -> Result<HashMap<ModelId, ConfiguredModelRoute>, InferenceError>
    加载全局模型路由：私有函数
    行为：
        有界读取UTF-8 TOML，要求模型列表非空，model id唯一
        校验api_type工厂、http/https端点、凭据及Provider选项，禁止将凭据写入日志
        解析context_window的k/m/b/t无符号整数后缀，值必须大于0，缺省沿用1m
        所有条目编译成功才返回新map，部分失败不发布残缺配置
```

## 逻辑

```text
请求主线：
    MCL参数快照 -> InferenceRequest -> 主线程模型/资源/发送器快照
    通用异步闭包 -> Provider HTTP与SSE -> 直接向已收集webui发送器推流
    完整assistant结果 -> oneshot -> MCL异步闭包 -> ECS收口并写assistant历史 -> MclCommandResult

生命周期：
    每次请求使用Agent.execution派生CancellationToken，在同一AgentExecutionScope登记Inference活动，不建立InFlightInferences全局表
    Driver/Agent停止关闭整个作用域受理门，cancel全部活动并等待Inference、Sandbox、Other的真实cleanup及MCL命令实际收口完成，成功前不能销毁Agent或重建作用域
    发出取消不等于任务已退出，该等待能力必须由任务所属执行设施提供，不能用oneshot通道关闭假称已清理
    HTTP任务运行于通用异步执行设施，不阻塞Workspace Lua本地线程
    Agent销毁后的快照仍不允许回写Agent；MCL收口核对Entity，旧结果不得命中新实例

删除旧入口：
    不保留CapturedInference、ContextCompaction专门请求及输出路由
    不保留PreparedInference事件、固定异步System、publish_inference_output_system
    不保留CancelInference按表查询事件、AgentFailure或Lua邮箱输出
    若将来需要压缩，调用同一推理接口，由调用方解释结果
```

# events

## 类型

公开，统一定义于共享types的inference_events.rs；包括内部重载输出和监督错误，不保留插件events.rs：
```text
InferenceRequest：统一推理请求，公开事件
    id: String--本次推理请求唯一关联，不是turn代号
    agent: Entity--完整Agent实例
    messages: Vec<AgentMessage>--拥有所有权的消息快照
    resources: Vec<ResourceID>--本次模型可见资源地址
    cancel: CancellationToken--Agent.execution共享作用域派生取消令牌，准备时校验归属，不接受独立根令牌绕过停止
    reply: Arc<Mutex<Option<oneshot::Sender<Result<InferenceResponse, InferenceError>>>>>--可Clone请求共享唯一Sender
    impl Event for InferenceRequest

InferenceResponse：完整推理响应，公开共享结构体--oneshot成功载荷，不是第二个结果事件
    message: AgentMessage--assistant完整消息，含既有reasoning、tool_calls和usage
    stop_reason: StopReason--保留Provider停止原因

ReloadModelRoutes：全局配置重载请求，公开事件
    id: String--请求关联
    impl Event for ReloadModelRoutes

ReloadModelRoutesResult：全局配置重载结果，公开事件
    id: String--原请求ID
    result: Result<usize, InferenceError>--成功为模型路由数量
    impl Event for ReloadModelRoutesResult

InferenceReloadOutput：配置重载输出，公开共享内部类型
    id: String--原重载请求ID
    result: Result<HashMap<ModelId, ConfiguredModelRoute>, InferenceError>--完整候选配置

InferenceReloadTaskError：配置重载监督错误，公开共享类型
    id: String--必须保留重载关联
    source: AsyncTaskError--通用执行错误
    说明：From<AsyncTaskError>不能自动取得id，提交包装须保证关联；不可丢失请求仅记日志。
```

## 逻辑

```text
共享类型归属：
    InferenceResponse依赖的AgentMessage、StopReason及InferenceError为共享纯值契约，不反向依赖InferencePlugin
    模型路由纯值与Provider公开trait统一由共享inference_types提供；所有公开trait签名中的输入、请求、响应、累积器、增量、擦除别名及错误都必须公开可见，不能引用插件私有类型；协议具体实现仍在插件types
    无领域World扩展，本次接口用事件即可，不新增WorldInferenceExt或Workspace路由配置方法
    本文仅设计目标源码归属，实际文件迁移尚未进行
```

# types

## 类型

公开：
```text
ModelId：模型选择标识，公开值类型
    value: String--非空、不超过256字符、不含控制字符

ConfiguredModelRoute：已编译全局路由，公开纯值类型
    model: String--Provider模型名
    context_window_tokens: u64--上下文窗口信息，不假称可精确估算全部token
    adapter: ErasedProviderAdapter--不可变协议适配器快照，内含端点及凭据

InferenceParameters：采样参数，公开纯值类型
    temperature: Option<f32>
    max_output_tokens: Option<u32>
    top_p: Option<f32>
    stop: Vec<String>
    validate(&self) -> Result<(), InferenceError>
        验证采样参数：公开方法
        行为：验证有限浮点及协议允许范围、非零输出上限、停止序列数量与长度；Provider特有不支持参数由Adapter拒绝。

ToolDefinition：模型工具定义，公开共享值类型
    name: String--资源alias
    description: String--资源描述
    input_schema: serde_json::Value--输入schema

ProviderInput：协议无关输入，公开纯值类型
    model: String
    parameters: InferenceParameters
    messages: Vec<ProviderMessage>--规范化后的消息
    tools: Vec<ToolDefinition>

ProviderMessage：协议消息，公开纯值契约
    说明：表达用户、assistant、工具结果和必要Provider角色；不包含Agent路由事件或inject容器。

ProviderRouteInput：已解析路由输入，公开纯值结构体
    provider: Option<String>
    base_url: Url
    api_key: String--禁止日志输出
    thinking: Option<String>
    reasoning_effort: Option<String>

ProviderAdapterFactory：协议工厂，公开trait
    约束：Send + Sync + 'static
    build(&self, config: &ProviderRouteInput) -> Result<ErasedProviderAdapter, InferenceError>
        创建协议适配器：公开trait方法
        行为：检查端点、凭据和api_type特有选项，返回不可变共享Adapter。

ProviderAdapter：协议适配器，公开trait
    约束：Send + Sync + 'static
    build_request(&self, input: &ProviderInput) -> Result<ProviderHttpRequest, InferenceError>
        编码HTTP请求：公开trait方法
        行为：将统一消息、schema和参数转换为Provider协议，不访问World。
    begin_response(&self, status: StatusCode, headers: &HeaderMap) -> Result<Box<dyn ProviderResponseAccumulator>, InferenceError>
        建立响应累积器：公开trait方法
        行为：检查响应格式，为每次请求创建独立累积器。

ProviderResponseAccumulator：响应累积器，公开trait
    约束：Send + 'static
    push(&mut self, chunk: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError>
        累积响应分片：公开trait方法
        行为：处理跨分片字节与SSE结构，返回有序新增reasoning和正文。
    finish(self: Box<Self>) -> Result<(ProviderInferenceResponse, Vec<ProviderStreamDelta>), InferenceError>
        完成累积：公开trait方法
        行为：处理尾部并验证完整结束，返回最终响应及尚未推送的尾部增量，不把断流当完成。

ProviderHttpRequest：已编码请求，公开纯值结构体
    method: Method
    url: Url
    headers: HeaderMap
    body: Vec<u8>

ProviderInferenceResponse：完整Provider响应，公开纯值结构体
    reasoning: Option<String>
    content: Option<String>
    tool_calls: Vec<ToolCall>
    usage: Option<TokenUsage>
    stop_reason: StopReason

ProviderStreamDelta：增量，公开纯值枚举
    Reasoning(String)
    Content(String)

StopReason：停止原因，公开共享枚举
    Completed
    ToolCalls
    Length
    ContentFilter
    Other(String)

ErasedProviderAdapter = Arc<dyn ProviderAdapter>：协议适配器，公开类型别名
ErasedProviderAdapterFactory = Arc<dyn ProviderAdapterFactory>：协议工厂，公开类型别名
```

私有：
```text
PreparedInference：闭包捕获快照，私有结构体--不是事件
    id: String
    agent: Entity
    agent_id: ResourceID
    agent_instance: String--主线程生成的不透明实例关联，不接受外部请求以此寻址
    input: ProviderInput
    adapter: ErasedProviderAdapter
    client: reqwest::Client
    senders: Vec<WebSocketSender>
    cancel: CancellationToken
    activity: ExecutionActivity--共享作用域登记的Inference活动句柄，真实cleanup后mark_completed
    reply: Arc<Mutex<Option<oneshot::Sender<Result<InferenceResponse, InferenceError>>>>>
    limits: InferenceLimits

InferenceLimits：安全上限，私有结构体
    config_bytes: usize--默认1MiB
    message_bytes: usize--默认单项4MiB
    messages_bytes: usize--默认总量16MiB
    tool_count: usize--默认256
    tool_description_bytes: usize--默认8KiB
    error_body_bytes: usize--默认16KiB
    response_bytes: usize--必须有界，最终值实现前确定
    inject_depth: usize--必须限制递归，最终值实现前确定
    request_timeout: Duration--覆盖整个请求，默认值实现前确定
    stream_send_timeout: Duration--慢客户端隔离上限，默认值实现前确定

ModelRouteConfig：模型配置项，私有结构体
    id: String
    model: String
    provider: Option<String>
    base_url: String
    api_key: String
    api_type: String
    thinking: Option<String>
    reasoning_effort: Option<String>
    context_window: Option<String>

OpenAiAdapter与DeepSeekAdapter：协议实现，私有类型
    说明：复用协议共同结构，但保持DeepSeek特有语义，不按base_url猜测api_type。

OpenAiAccumulator：SSE累积器，私有结构体
    buffer: Vec<u8>--跨分片保留，单行及总量有界
    reasoning: String
    content: String
    tool_calls: BTreeMap<usize, ToolCallBuilder>--按Provider index累积
    usage: Option<TokenUsage>
    stop_reason: Option<StopReason>
    completed: bool--已观察到协议完整结束
```

## 逻辑

```text
OpenAI协议：
    使用chat/completions流式请求，工具name原样为alias，stream_options.include_usage开启
    SSE按完整行处理，支持CRLF、多data行及跨UTF-8分片，未完整的字节保留到下一分片
    正文、reasoning与工具参数按delta累积，工具调用按index合并ID、name和arguments片段
    usage尾部无choices也必须处理，统一input/output与prompt/completion计数及缓存命中字段
    完整结束后检查工具ID唯一、名称存在及参数结构，不生成半条工具调用

DeepSeek协议：
    thinking显式disabled必须写入请求，不能省略后误用模型默认思考模式
    启用或未明确禁用思考时，assistant回传reasoning_content，缺内容补空串
    显式disabled时仍按工具调用协议处理必要reasoning字段
    校验thinking与reasoning_effort组合，不支持的组合在准备阶段失败

边界与保密：
    不自动截断输入、不自动压缩、不自动继续Length响应、不自动执行tool_calls
    HTTP非成功状态有界提取error.message等Provider细节，控制字符单行化，消息截断
    日志URL只保留安全origin，不记录API key、Authorization或完整含密钥请求
    不默认记录完整prompt；用户MCL历史由MCL负责，不在推理日志复制一份
```

# error

## 类型

公开，共享推理错误契约：
```text
InferenceErrorKind：推理错误分类，公开枚举
    InvalidRequest
    AgentNotAlive
    InvalidModelId
    ModelRouteNotFound
    InvalidParameters
    InvalidMessages
    InvalidToolDefinitions
    ResourceMissing
    UnsupportedResource
    UnsupportedInput
    ConfigReadFailed
    ConfigDecodeFailed
    InvalidModelRoute
    UnsupportedApiType
    ConfigBusy
    SubmitFailed
    RequestFailed
    ResponseStatus
    ResponseDecodeFailed
    ResponseIncomplete
    ResponseTooLarge
    Timeout
    Cancelled
    TaskPanicked
    TaskAborted

InferenceError：推理错误，公开结构体
    kind: InferenceErrorKind
    message: String--不超过512字节，按UTF-8字符边界截断
    status: Option<u16>--HTTP状态码
    impl Clone + Debug + PartialEq + Eq for InferenceError
    impl fmt::Display for InferenceError
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：公开trait方法
            行为：输出分类、可选HTTP状态和有界描述，不输出凭据。
    impl std::error::Error for InferenceError
```

## 逻辑

```text
任何失败都通过原请求oneshot完成，不能只记warning后静默返回
客户端发送失败是推流旁路失败，不转换为模型失败；HTTP或协议失败则返回InferenceError
收到部分增量后失败不会撤回已显示内容，终态应明确失败；没有自动重试避免重复内容与计费
作用域取消和真实cleanup等待采用已批准的共享AgentExecutionScope契约，不再作为待定方案；Client快照使用WorldClientExt.client_webui_senders()；安全上限默认值仍须实施时落实
```
