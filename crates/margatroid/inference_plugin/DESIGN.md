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

**本文描述已实现的typed snapshot请求：InferenceRequest.tools为Vec<CallableResource>，System消息保留真实Provider role。** 既有单推理入口、准备System、通用异步闭包及独立supervisor/HTTP worker、oneshot与真实cleanup边界保持不变；现消费MCL选定的不可变typed snapshots。全局仍只有一份模型配置；提交前取得全部已注册webui客户端发送器快照，闭包直接推流，后来连接不加入本次推理。

当前输入边界：MCL只有MESSAGE、TOOL、SANDBOX三种typed arrays。TOOL保存可调用资源值（ResourceID、alias、description、schema及不可变共享执行材料快照），不是ToolCall请求数组。`IMPORT TOOL tool/skill/hook AS alias`绑定资源，`GET alias`解析并取得typed snapshot；`infer`参数依次为MESSAGE array、TOOL array，不接受ResourceID array，不再次执行global alias query。旧Block不随reload暗变，新GET alias取得新快照。Hook允许导入TOOL，但Driver触发器与执行路线尚未闭合，不假定其为普通Lua工具。

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
                创建共享HTTP客户端，插入InferenceConfig；该Resource同时作为重复安装检查
                只挂载inference_system，不安装固定异步System或结果发布System

InferenceConfig：推理配置，公开Resource--实现侧配置，不是图书馆组件
    path: PathBuf--唯一模型配置路径
    routes: HashMap<ModelId, ConfiguredModelRoute>--全局路由
    factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>--重载时使用的工厂
    client: reqwest::Client--共享连接池
    schedule: String--通用异步闭包投递计划
    reload_id: Option<String>--当前全局配置重载关联，私有字段；初始None
    model_count(&self) -> usize
        路由数量：公开方法，返回当前完整路由数。
    get(&self, id: &ModelId) -> Option<&ConfiguredModelRoute>
        路由查询：公开方法，返回当前路由引用。
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
    处理事件：InferenceRequest、ReloadModelRoutes、InferenceReloadOutput
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
        先take共享原回复Sender并转入私有回复槽，克隆重复请求不再执行；捕获准备panic；校验请求ID（非空、至多256字节、无控制字符）、完整Agent Entity及取消状态；已失效或取消直接完成oneshot错误
        复制Agent地址、info.model中的模型与采样参数，从全局配置查唯一模型路由
        从request.tools的CallableResource不可变共享快照准备typed tool definitions；对各snapshot自带ResourceID使用validate_owner验证目标Agent ResourceSet存在、image_dependencies许可及ImportBindings的ID/TOOL kind已导入归属，不按ID重取latest、不再次global alias query
        完整校验alias唯一、定义与schema后规范化消息，验证参数与输入上限；旧Block在资源reload后仍使用选定的旧定义快照
        收集全部当前webui发送器快照，复制HTTP客户端、路由及协议适配器
        保存拥有所有权的PreparedInference，不携带World、组件借用或Workspace配置
        通过Agent.execution共享AgentExecutionScope受检入口登记Inference活动，令牌从该作用域派生；登记与沙箱更新复用同一互斥门，但Inference活动不阻止沙箱改变
        建立一次性回复保障，提交执行闭包；准备或提交失败均完成原回复槽，提交失败撤销未启动活动登记
        不要求Agent.turn非空，不用turn代号作为推理请求唯一ID

handle_reload_model_routes(world: &mut World, request: ReloadModelRoutes)
    发起全局配置重载：crate公开函数
    行为：
        校验ID，若本次重载仍在途则返回ConfigBusy，不并行发布不同版本
        复制唯一配置路径及调度计划，记录重载请求关联，异步有界读取并解析TOML，输出拥有所有权的原始String；收口阶段完整编译全部协议Adapter
        返回InferenceReloadOutput{id,result:Result<String,InferenceError>}；ReloadGuard保证丢弃路径仍带id投递失败；读取、解析或监督失败都保留旧配置
        只重载全局配置，不接受workspace或项目路径覆盖

handle_model_routes_reloaded(world: &mut World, output: InferenceReloadOutput)
    收口配置重载：crate公开函数
    行为：
        核对本次重载关联，重复或迟到结果不替换其他版本
        对原始String在主线程完整编译并捕获工厂panic，成功整体替换routes，失败保持原配置；清除本次重载标记
        发送关联原请求的ReloadModelRoutesResult，已开始推理继续使用自身路由快照
```

私有：
```text
collect_tool_definitions(world: &World, agent: Entity, tools: &[CallableResource]) -> Result<Vec<ToolDefinition>, InferenceError>
    准备可见工具定义：私有函数
    行为：
        消费TOOL array已选定的CallableResource快照；逐项以快照内完整ResourceID进行validate_owner，World只用于ResourceSet存在、依赖许可及已导入ID/kind校验，不要求最新alias或定义与旧snapshot一致，不遍历global alias集合、不按ResourceID重新读取最新ResourceSet定义
        模型工具名使用snapshot.alias；description、input_schema使用同一snapshot的准备值，不从当前重载资源重新生成
        tool保存已加载输入schema；skill快照采用固定schema {"type":"object","properties":{},"additionalProperties":false}（参数仅{}），与ToolPlugin校验一致，不要求skill包增加schema文件；准备阶段不执行脚本
        hook允许导入TOOL并消费其已准备typed定义；ToolPlugin执行明确返回UnsupportedHook；这不代表Driver触发器执行路线已确定，更不赋予普通Lua runner执行语义；prompt/sandbox不能冒充TOOL
        归属无效、缺乏模型工具定义、重复alias、非法工具名或不合法schema均在HTTP提交前拒绝，不悄悄忽略；空tools表示无工具，不回退默认集合
        CallableResource共享快照不可变；资源reload只影响后续GET alias，不能改变已有Block或在途推理定义

normalize_messages(messages: Vec<AgentMessage>) -> Result<Vec<ProviderMessage>, InferenceError>
    规范化输入消息：私有函数
    行为：
        inject递归按顺序展开内部AgentMessage数组，限制嵌套深度及展开后的数量和字节数
        System、user、assistant、tool_call按各自真实role转为对应Provider消息；prompt导入保留其真实role，不能一律改为user；error转用户错误文本
        System是完整AgentMessage变体，infer可接收System；history append的独立允许范围不含System，不能用该限制缩窄infer
        保留全部消息已有resource_id及工具调用ID、assistant reasoning和参数；Provider编码保留角色与调用关联，内部ID不因编码或历史收口丢失
        历史usage不作为Provider请求内容；未知或无法表达的结构返回错误，不静默丢弃
        检查工具调用与结果的关联及必要字段，空输入及超限拒绝

prepare_inference内联收集发送器：没有独立collect_webui_senders函数
    行为：调用WorldClientExt.client_webui_senders()，去重，忽略已断开的连接；取得本次快照，不保存全局推理订阅表。

run_provider(prepared: PreparedInference) -> Result<InferenceResponse, InferenceError>
    执行模型请求：私有异步函数
    行为：
        HTTP worker只拥有本次请求/发送器；取消、回复Receiver关闭与180秒整体超时由supervisor监督，不无限等待网络
        发送主线程已通过Adapter编码的HTTP请求，校验状态；失败有界读取错误body
        成功时创建本次ResponseAccumulator，逐分片解析并累积assistant、工具调用和usage
        解析到正文或reasoning增量就直接推送快照中的发送器，不发领域增量事件再中转
        finish必须确认协议完整结束并推送尾部增量；网络提前结束、非法分片或超限返回错误
        完整工具调用即使没有正文也可成功；Length返回已有结果及截断原因，不自动续写
        ContentFilter保留真实停止原因；不把残缺网络响应当作正常截断
        验证返回工具名称属于本次可见alias集合，生成带既有内容及usage的assistant AgentMessage和stop_reason，返回完整结果
        不查询Agent，不写历史，不执行工具，不重试请求

execute_inference(prepared: PreparedInference, guard: TaskGuard)
    执行设施入口：私有异步函数
    行为：
        创建私有owner_cancel和DropGuard，spawn独立supervise_inference并await其JoinHandle
        外层闭包丢弃只取消owner_cancel，不丢弃仍需执行cleanup的supervisor
        supervisor正常结束后disarm owner DropGuard

supervise_inference(prepared: PreparedInference, guard: TaskGuard, owner_cancel: CancellationToken)
    推理监督：私有异步函数
    行为：
        mark_started后spawn拥有HTTP/SSE/增量发送future的run_provider worker
        合并活动child、请求cancel、owner取消、回复Receiver关闭（20ms检查）与180秒整体超时
        正常worker结果通过JoinHandle收取；panic转TaskPanicked
        其它分支显式worker.abort后await worker JoinHandle，取得future已退出及资源已释放的证明
        向原快照发送器有界发送agent.message.finished；owner取消可中断终态发送并释放其future
        HTTP worker join且终态future释放后才mark_completed，再take私有Sender发送一次Result
        mark_completed失败标cleanup_failed并回复错误；不关闭Sender冒充真实cleanup

push_stream(senders: &mut Vec<WebSocketSender>, id: &str, agent_instance: &str, agent_id: &ResourceID, sequence: u64, delta: ProviderStreamDelta)
    直接推送增量：私有异步函数
    行为：
        保留正文与reasoning不同消息种类，携带请求ID、Agent地址、不透明agent_instance及递增序号；内部完整Entity只用于实例核对，不作为推流DTO字段或外部请求目标
        使用客户端现有消息编码契约，发送器快照从开始到结束不再补充后来连接
        关闭发送器从本次活动列表剔除；客户端失败不改变模型结果
        不因慢客户端无限阻塞模型读取，各连接并发100ms发送超时并停止向该慢连接推送
        不逐增量写历史，不把未完整的工具参数当作可执行调用
        成功或失败向快照发送器发送agent.message.finished {id,agent,agent_instance,error:null|{kind,message}}；oneshot仍是唯一权威完成结果

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
    HTTP worker及独立supervisor运行于通用异步闭包所在Tokio运行时，不阻塞Workspace Lua本地线程
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
    tools: Vec<CallableResource>--TOOL array选定的可调用资源不可变共享快照，自带resource_id、alias及definition，不是ResourceID array或ToolCall array
    turn: Option<String>--调用方关联快照，不作为请求唯一ID；推理不要求非空
    cancel: CancellationToken--请求取消令牌；现CancellationToken接口无法检查父子归属，supervisor始终同时监督独立登记活动的scope child，不能绕过Agent停止
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

InferenceReloadOutput：配置重载输出，公开共享内部事件
    id: String--原重载请求ID
    result: Result<String, InferenceError>--已通过有界UTF-8读取及TOML解析的原始配置；不携带插件Adapter
    impl Event for InferenceReloadOutput
    说明：原始文本包含配置凭据，不序列化到客户端、不记录日志；主线程编译完成后整体发布。
```

## 逻辑

```text
共享类型归属：
    InferenceResponse依赖的AgentMessage、StopReason及InferenceError为共享纯值契约，不反向依赖InferencePlugin
    模型路由、Provider公开trait及协议辅助值仍在插件src/types.rs；ProviderMessage公开且协议无关；签名中全部类型公开可见，错误及AgentMessage/ToolCall/TokenUsage/StopReason来自共享types；不反向依赖插件
    无领域World扩展，本次接口用事件即可，不新增WorldInferenceExt或Workspace路由配置方法
    共享事件已迁移；插件没有events.rs，src/error.rs仅重新导出共享错误
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
    adapter: ErasedProviderAdapter--不可变协议适配器快照，内含端点及凭据；字段crate公开，公开只读accessor

InferenceParameters：采样参数，公开纯值类型
    temperature: Option<f32>
    max_output_tokens: Option<u32>
    top_p: Option<f32>
    stop: Vec<String>
    validate(&self) -> Result<(), InferenceError>
        验证采样参数：公开方法
        行为：验证有限浮点及协议允许范围、非零输出上限、停止序列数量与长度；Provider特有不支持参数由Adapter拒绝。

CallableResource：TOOL元素，公开共享纯值契约--定义归共享types，本插件消费而不另建副本类型
    resource_id: ResourceID--导入资源完整身份，validate_owner仅校验已导入及许可，不取得新版本
    alias: String--导入绑定的模型可见alias
    definition: Arc<CallableDefinitionSnapshot>--不可变共享定义与执行材料，包含description、input_schema及tool/skill/hook种类材料；clone不复制或刷新版本
    说明：Agent ImportBindings为权威alias绑定；不新增插件资源注册表。GET alias得到当前绑定的新快照，已有Block保留旧snapshot。

ToolDefinition：模型工具定义，公开共享值类型
    name: String--选定CallableResource.alias
    description: String--选定definition的描述
    input_schema: serde_json::Value--选定definition的输入schema

ProviderInput<'a>：协议无关输入借用，公开结构体
    model: &str
    parameters: &InferenceParameters
    messages: &[ProviderMessage]--规范化后的消息借用
    tools: &[ToolDefinition]
    说明：ProviderInput带生命周期，主线程编码request后不进入异步任务。

ProviderMessage：协议消息，公开纯值契约
    说明：明确表达System、用户、assistant和工具结果真实角色；不包含Agent路由事件或inject容器，不能把System降格为用户消息。

ProviderRouteInput<'a>：已解析路由输入借用，公开结构体
    provider: Option<&str>
    base_url: &Url
    api_key: &str--禁止日志输出
    thinking: Option<&str>
    reasoning_effort: Option<&str>

ProviderAdapterFactory：协议工厂，公开trait
    约束：Send + Sync + 'static
    build(&self, config: ProviderRouteInput<'_>) -> Result<ErasedProviderAdapter, InferenceError>
        创建协议适配器：公开trait方法
        行为：检查端点、凭据和api_type特有选项，返回不可变共享Adapter。

ProviderAdapter：协议适配器，公开trait
    约束：Send + Sync + 'static
    build_request(&self, input: ProviderInput<'_>) -> Result<ProviderHttpRequest, InferenceError>
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
    agent_id: ResourceID
    agent_instance: String--主线程format!("{:?}",完整Entity)，与Protocol快照一致，仅作不透明关联；不接受入站解码寻址
    request: ProviderHttpRequest--主线程已编码请求，不再携带ProviderInput借用
    adapter: ErasedProviderAdapter
    client: reqwest::Client
    tool_names: HashSet<String>--本次可见alias，用于最终工具名称校验
    senders: Vec<WebSocketSender>
    cancel: CancellationToken--请求取消
    reply: InferenceReply--已从原共享Sender认领的私有一次性槽
    说明：没有agent Entity、activity、limits或ctx字段；测试编译独有terminal_gate用于确定性终态abort测试。

TaskGuard：活动及回复保障，私有结构体
    activity: ExecutionActivity
    reply: InferenceReply
    finished: bool
    started: bool
    impl Drop for TaskGuard
        未启动则rollback_submission；已启动且supervisor本身异常退出则mark_cleanup_failed并保留登记
        Drop不证明cleanup，不以关闭回复Sender假释放活动；正常流程由supervisor显式join后mark_completed

ReloadGuard：重载回复保障，私有结构体
    id: String
    events: RuntimeEventSender
    finished: bool
    complete(self, result: Result<String, InferenceError>)
        投递关联输出并关闭保障；Drop未关闭则投递带id的TaskAborted错误

安全上限：编译期常量及执行函数固定值，不存在InferenceLimits结构体
    config_bytes: 1MiB--File.take(limit+1)有界读取
    message_bytes: 4MiB--单条序列化消息及单项响应文本/工具参数
    messages_bytes: 16MiB--展开消息总序列化字节
    messages_count: 4096
    inject_depth: 32
    tool_count: 256--输入工具数及响应tool index
    tool_name_bytes: 64
    tool_description_bytes: 8KiB
    stop_count: 64
    stop_bytes: 256
    error_body_bytes: 16KiB
    response_bytes: 32MiB--HTTP与累积器总字节都有界
    sse_buffer_bytes: 4MiB--跨行缓冲与单事件data
    request_timeout: 180秒
    stream_send_timeout: 100ms--每连接并发发送
    receiver_closed_poll: 20ms

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
    buffer: Vec<u8>--跨分片保留，4MiB上限；总流量32MiB
    reasoning: String
    content: String
    event_data: Vec<u8>--多data行拼接
    total_bytes: usize--累计原始流量上限
    tool_calls: Vec<OpenAiToolCallBuilder>--按有界Provider index累积
    usage: Option<TokenUsage>
    stop_reason: Option<StopReason>
    saw_choice: bool--至少观察到一个choice
    done: bool--已观察到[DONE]；finish必须同时有choice、停止原因与done
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
    传输错误URL只保留安全origin与分类，不复制可能含凭据的底层错误链，不记录API key、Authorization或完整含密钥请求
    不默认记录完整prompt；用户MCL历史由MCL负责，不在推理日志复制一份
```

# error

## 类型

公开，共享推理错误契约：
```text
InferenceErrorKind：推理错误分类，公开枚举
    InvalidRequest
    AgentMissing
    PreparationFailed
    ExecutionFailed
    ResponseLost
    AgentNotAlive
    InvalidModelId
    ModelRouteNotFound
    InvalidParameters
    InvalidMessages
    InvalidToolDefinitions
    ResourceMissing
    UnsupportedResource
    UnsupportedInput
    DuplicateModelId
    ConfigReadFailed
    ConfigDecodeFailed
    InvalidModelRoute
    UnsupportedApiType
    ConfigBusy
    SubmitFailed
    RequestBuildFailed
    RequestFailed
    ResponseStatus
    ResponseDecodeFailed
    ResponseEncodeFailed
    ResponseIncomplete
    ResponseTooLarge
    Timeout
    Cancelled
    TaskPanicked
    TaskAborted

InferenceError：推理错误，公开结构体
    kind: InferenceErrorKind
    message: String--不超过512字节，按UTF-8字符边界截断
    说明：无status字段，with_status将HTTP状态写入有界message；kind()/message()提供只读accessor。
    impl Clone + Debug + PartialEq + Eq for InferenceError
    impl fmt::Display for InferenceError
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：公开trait方法
            行为：输出分类和有界描述（HTTP状态已在message内），不输出凭据。
    impl std::error::Error for InferenceError
```

## 逻辑

```text
任何失败都通过原请求oneshot完成，不能只记warning后静默返回
客户端发送失败是推流旁路失败，不转换为模型失败；HTTP或协议失败则返回InferenceError
收到部分增量后失败不会撤回已显示内容，终态应明确失败；没有自动重试避免重复内容与计费
作用域取消和真实cleanup等待采用已批准的共享AgentExecutionScope契约，不再作为待定方案；Client快照使用WorldClientExt.client_webui_senders()；安全上限见types板块既有固定值

当前实现与验收边界：
    InferenceRequest.tools为Vec<CallableResource>，消费选定typed definition与resource_id validate_owner，不接受ResourceID array或重复global alias query
    System消息及prompt真实role可完整规范化，infer范围不被history append不含System的独立限制缩窄；输入与历史既有resource IDs保留
    重复alias、无效归属或定义失败发生于HTTP提交前；reload不暗改旧Block/在途定义，新GET alias得到新snapshot
    tool_call effect以单assistant MESSAGE array + TOOL visible array取assistant.tool_calls，空calls返回emptyMessage、不再次记录assistant；alias冲突/重复call ID/多条或非assistant先失败且不部分submit
    Hook允许导入及提供typed定义，但Driver触发器/执行路线仍未闭合，不宣称普通Lua执行；工具脚本/skill正文磁盘材料版本隔离也需另行闭合，不以metadata snapshot保证冒充该能力
    现有测试验证HTTP worker abort并join、终态future释放后mark_completed的真实cleanup，而非仅oneshot关闭
```
