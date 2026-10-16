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

**当前已实现MESSAGE/TOOL/SANDBOX typed数组、System消息、Agent全域alias绑定、真实SQLite Block、共享异步storage及持久ID resolve。Agent只保存database及同连接历史视图，不持有Block内容；数据库TOOL/SANDBOX不保存定义/policy快照。ToolCall纯输入仍保存在Assistant.tool_calls，不是独立Block类型。finish_turn已实现，旧finish拒绝。**

共享值、组件与事件迁移及本轮typed数组与导入绑定均已实现。各领域文档描述对应完整行为，本文件统一依赖方向和跨领域数据，不继续混入旧邮箱、引用Block或ResourceId组件方案。ResourceID等基础契约由resource_id_plugin提供；types按组件、事件与纯值模块组织，不依赖业务Plugin实现。正文保持lib、system、handler、events、types、error六个标准板块。

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。history_components::message_history_parts将实际正文放入HistoryRecord.content，payload保留type及其余字段并移除顶层content；Error正文取message并从payload移除，Inject正文为空且完整保留嵌套messages。Assistant无正文按空字符串展示。Assistant.stop_reason为Option<StopReason>，旧文件缺字段默认None；模型实际输出始终Some，历史payload保留该元数据。旧行不迁移，消费者兼容旧payload.content/message。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。

## 用户轮次中止契约（已实现）

无 `get_turn`。Agent状态为 Creating / Idle / Working / Stopping，对外同步为 `creating/idle/working/stopping`，同时发布 turn 与 status。启动成功进入 Idle、turn=None；start_turn仅允许Idle，原子创建唯一turn并置Working，不覆盖已有轮次。AbortAgentTurn控制系统在RuntimePlugin::PRE_UPDATE执行，先于UPDATE中的MCL实际动作及Inference/Tool受理；自定义调度必须保留该顺序。控制受理验证完整Agent实例、workspace、Driver实例及expected_turn_id，立即清空turn但保持Working，仅取消匹配活动的独立turn_cancel，不关闭根作用域。正常finish_turn核对当前Driver结果关联及真实清理完成后才置Idle；Working收口期间同一中止关联重复请求幂等，不能让迟到旧完成覆盖新轮次。用户轮次中止不等于根Agent shutdown、owner abort、timeout或receiver closed。

# lib

## 逻辑

```text
依赖方向：
    共享types只依赖基础ECS、通用执行接口和纯值依赖，不依赖业务Plugin实现
    ResourceID和ResourceSet等基础契约必须位于共享层或共享层可依赖的纯基础crate
    Agent、Workspace、Client、资源条目及World扩展声明与实现定义在对应xxx_components.rs
    Plugin、System和Handler留在实现crate；文档仍在lib板块描述共享组件，不按源码迁移改变文档板块
    Provider公开输入与trait目前位于InferencePlugin的types模块，只引用共享消息纯值；共享事件通过原始配置文本传递重载，不反向依赖这些协议实现
    具体SQLite、HTTP、Lua VM、runner和平台沙箱实现不进入共享组件
```

# system

## 逻辑

```text
无System，不做消息投递、任务轮询、GC、数据库同步或事件转发。
```

# handler

## 逻辑

```text
无业务Handler。共享World扩展的受检实现属于对应components文件，不能留在Plugin使消费者依赖实现crate。
AgentExecutionScope拥有活动受理、取消和完成等待的共享机制，不包含具体工具、HTTP或数据库执行器。
```

# events

## 逻辑

```text
事件归属：
    所有公开和内部事件定义于对应xxx_events.rs，插件不保留events.rs
    自动异步输出为框架Result<T,E>，T/E及其关联载荷也在共享events定义
    mcl_events：MclCommand、MclCommandResult、MclAsyncOutput及错误关联
    inference_events：InferenceRequest、InferenceResponse、配置重载请求及关联输出
    tool_events：ToolExecuteRequest、ToolExecuteResponse
    agent_events：启动/停止及AgentException、AgentExceptionResult
    workspace_events：启动/停止及WorkspaceException、WorkspaceExceptionResult
    lua_runtime_events：Driver启动/停止、异常及对应结果，ended仍是oneshot槽位不是额外事件
    各资源events：加载请求、一次性回复槽、公开观察Result及读取输出
    AgentDriver保留Workspace依赖的加载Result；完成保障是私有实现，不建立跨插件LoadCompletion公共trait

完成通道：
    oneshot：一次请求完成，Clone事件共享同一个Arc<Mutex<Option<Sender>>>槽
    框架Result：闭包到ECS的结果收口，需要World回写时使用
    观察事件：不作为队列推进、清理或执行成功的依据
    异常链：报告处理事实，不替代请求结果，不消费Result后重新发同类异常造成循环

移除旧入口：
    删除旧AgentMessage路由事件、AgentFailure、AgentControl、Lua消息邮箱封装
    删除CapturedInference及ContextCompaction专用请求、旧ToolCallEvent和历史转发请求
    不保留RefBlock、RefMerge或ResourceRef作为目标值；迁移兼容代码不构成新契约
```

# types

## 类型

公开：
```text
ResourceID：资源地址，公开纯值类型
    说明：不是Component；图书馆身份存在组件中，资源ID仅存在ResourceSet的map key中。

ToolCall：工具调用输入，公开结构体
    id: String--单批次唯一的调用关联
    tool_name: String--模型看到的资源alias，不是全局身份
    arguments: String--完整JSON对象文本，空参数为{}
    impl Clone + PartialEq + Eq + Serialize + Deserialize for ToolCall

TokenUsage：单次推理统计，公开结构体
    input_tokens: u64
    output_tokens: u64
    cache_hit_tokens: u64--Provider未提供时为0

AgentMessage：消息值，公开枚举--不是Event，不包含路由Entity
    Inject { messages: Vec<AgentMessage> }
    System { content: String }--系统消息，prompt导入及推理按实际role保留
    User { content: String }
    Assistant { reasoning: Option<String>, content: Option<String>, tool_calls: Vec<ToolCall>, usage: Option<TokenUsage>, stop_reason: Option<StopReason> }
    ToolCall { resource_id: Option<ResourceID>, tool_call_id: String, tool_name: String, content: String, failed: bool }
    Error { message: String }
    impl Clone + PartialEq + Eq + Serialize + Deserialize for AgentMessage
    说明：JSON type为inject/system/user/assistant/tool_call/error；未知资源的可见性拒绝结果resource_id为None，不虚构builtin/invalid地址。
        inject递归包含同一消息值类型；history_append的允许范围独立校验，不能从消息枚举自动推导
        Assistant可只有合法工具调用而无正文；usage与新增stop_reason保留于结果及显式历史元数据，不回传作为模型输入
        目标stop_reason覆盖全部推理原因，新增StopReason::Cancelled序列化cancelled；兼容InferenceResponse.stop_reason由同一权威映射生成且两处始终相同
        用户轮次中止且真实cleanup成功返回Ok assistant：content="Turn aborted."、reasoning=None、tool_calls=[]、usage=None、stop_reason=Cancelled；不伪造部分执行成功
        cancelled assistant允许显式history_append，stop_reason保存在payload元数据而非content；不允许进入infer上下文（含Inject嵌套）
        infer接受System，prompt导入按资源实际role构造消息，不把所有prompt强制转换为System或User
        ToolCall结构体仍是Assistant.tool_calls的纯输入元素；AgentMessage::ToolCall是执行结果消息，两者都不构成独立TOOL_CALL Block或GET数组

CallableResource：可调用资源值，公开结构体--TOOL数组元素，不是裸ResourceID
    resource_id: ResourceID--完整来源身份
    alias: String--本次导入的模型可见alias快照
    definition: Arc<CallableDefinitionSnapshot>--共享不可变定义快照，不复制资源正文

CallableDefinitionSnapshot：可调用资源定义快照，公开枚举
    Tool(ToolResource)
    说明：复用共享资源定义，包含执行准备所需描述、schema及资源定义；GET TOOL从StoredBlockInner中的ID在当前已导入绑定中resolve最新定义，执行提交后不得再回查替换已提交快照。
        快照固定描述、schema和资源定义字段（包括路径），不冻结路径对应磁盘文件内容；执行时按既有路径读取，不宣称文件正文版本已被快照。

SandboxValue：沙箱资源值，公开结构体--SANDBOX数组元素
    resource_id: ResourceID--完整来源身份
    policy: Arc<str>--共享不可变策略快照；资源重载不暗改此值

ImportedValue：导入绑定值，公开共享枚举
    Message { resource_id: ResourceID, messages: Arc<[AgentMessage]> }--prompt来源ID及实际role消息快照
    Tool(CallableResource)--来源ID由CallableResource持有，不再重复
    Sandbox(SandboxValue)--来源ID由SandboxValue持有，不再重复
    说明：不建立重复资源正文仓库；MESSAGE共享消息快照，TOOL/SANDBOX共享Arc定义或策略。

ImportBindings：Agent导入绑定，公开共享类型--随Agent保存，不是额外Component或全局注册表
    bindings: BTreeMap<String, ImportedValue>--私有字段，alias到typed值的唯一权威绑定
    get(&self, alias: &str) -> Option<&ImportedValue>
        只读绑定：公开方法，不绕过World受检写入。
    contains_resource(&self, resource_id: &ResourceID, kind: InnerType) -> bool
        检查来源：公开方法，只检查ID与kind。
    get_resource(&self, resource_id: &ResourceID, kind: InnerType) -> Option<&ImportedValue>
        只读来源：按完整ID及kind取得本次绑定中的值，不加载资源。
    resolve_resource(&self, resource_id: &ResourceID, kind: InnerType) -> Result<BlockInner, AgentError>
        解析持久来源：在当前已导入绑定按完整ID/kind取得最新TOOL/SANDBOX定义；未导入或类型不符显式失败，不隐式加载，也不退回旧定义。
    说明：alias首字符必须是ASCII字母或下划线，后续仅ASCII字母、数字、下划线或连字符；不接受点、空白或非ASCII字符。
        受检更新与读取由共享agent_components中的WorldAgentExt统一实施，MCL只调用该契约。
        不按MESSAGE/TOOL/SANDBOX分别维护alias表；Agent全域同alias不同完整ID（含跨类型）拒绝。
        同ID同kind重载成功才替换对应alias的新值，失败保留旧绑定；重载alias改变时先查新alias冲突，成功时移除旧绑定再发布新绑定，不留下同来源不同alias分叉。
        alias/import bindings只留RAM，启动显式IMPORT恢复，不保存到数据库；每次GET持久ID都resolve当前最新已导入定义。
        已完成GET的值及已提交execution快照保持不变；activeSandboxValue不恢复，须GET再显式sandbox_use，重载不暗改已激活策略。

BlockInner：字段数组，公开枚举--只保留三种typed数组
    Message(Vec<AgentMessage>)--MESSAGE
    Tool(Vec<CallableResource>)--TOOL
    Sandbox(Vec<SandboxValue>)--SANDBOX
    inner_type(&self) -> InnerType
        读取类型：公开方法
        行为：返回对应变体类型，空数组也有类型。
    len(&self) -> usize
        读取长度：公开方法
        行为：返回数组元素数量。
    impl Clone + PartialEq + Eq + Serialize + Deserialize for BlockInner

InnerType：字段类型，公开枚举
    Message--MESSAGE
    Tool--TOOL
    Sandbox--SANDBOX
    说明：删除旧ResourceID/ToolCall变体；空数组仍携带上述具体类型，不能按长度猜测。

BlockPath：字段地址，公开结构体
    block_id: String
    inner_id: String

StoredBlockInner：数据库字段值，公开枚举--持久值与GET输出分离
    Message(Vec<AgentMessage>)--结构化消息JSON，保留完整消息字段
    Tool(Vec<ResourceID>)--只存完整来源ID，不序列化CallableDefinitionSnapshot
    Sandbox(Vec<ResourceID>)--只存完整来源ID，不序列化policy
    说明：空数组也有InnerType；rows为block definitions/fields及typed arrays，默认每field一行JSON，不强制每field独立SQL表。

BlockNamespace：数据库Block命名空间，公开枚举
    Persistent { agent: ResourceID }--稳定Agent逻辑身份，重启仍打开同一数据
    Temporary { instance: DriverLuaVmInstance }--完整启动instance隔离，旧实例重叠时不互见

BlockOpened：CREATE结果，公开结构体
    block_id: String--本次创建或打开的Block名称
    created: bool--仅本命名空间首次创建为true；open已有Block为false
    temporary: bool--区分CREATE BLOCK与CREATE TEMP BLOCK
    说明：默认CREATE BLOCK持久open/validate，不清空；同instance CREATE TEMP BLOCK也幂等open并检查schema，不清空。
        同一可见block名不同persistent/temporary lifecycle冲突拒绝，不因底层namespace不同静默选择另一Block。

AgentDatabaseStore：组合数据库接口，公开object-safe trait
    约束：AgentHistoryStore + AgentBlockStorage
    说明：共享层通过小型history adapter委托同一Arc，不依赖memory_plugin，也不要求trait upcasting。

AgentDatabaseHandle：Agent数据库能力，公开opaque可克隆句柄
    store: Arc<dyn AgentDatabaseStore>--私有；两个视图共享同一store及连接管理器
    new(store: impl AgentDatabaseStore) -> Self
        构造能力：公开关联函数，只包装backend，不执行SQL。
    history() -> AgentHistoryHandle
        取得同一SQLite文件的历史分表视图。
    block_storage() -> AgentBlockStorageHandle
        取得同一文件与connection manager的Block存储视图；Agent.database只保存此句柄，不保存Block内存内容。

AgentBlockStorage：共享Block存储接口，公开trait--Send + Sync + 'static
    execute(&self, request: BlockStorageRequest) -> Result<StoredBlockResult, BlockStorageError>
        阻塞机械操作：只在blocking worker调用；请求包含namespace、整次selectors/schema及INJECT必要检查，不由MCL拆成多个事务。

AgentBlockStorageHandle：异步Block存储句柄，公开可克隆类型
    execute(&self, request: BlockStorageRequest) -> impl Future<Output = Result<StoredBlockResult, BlockStorageError>>
        异步入口：调度blocking worker并等待真实join；所有Block只用真实SQLite backend，不提供RAM Block backend。
        GET在一次read transaction读取多selector同一snapshot；INJECT在同一write transaction读snapshot、执行map/type/range检查并replace，commit后才成功。
        取消只请求worker停止，不以Future丢弃伪造rollback；已commit明确返回已提交事实，无法确认commit则显式CommitUnknown，不声明未写入。

BlockStorageContext：完整请求命名空间上下文，公开结构体
    agent: ResourceID--稳定Agent身份
    instance: DriverLuaVmInstance--完整owner及启动关联；启动ID含进程session nonce，跨重启不复用TEMP

BlockSchema：创建声明，公开结构体
    name: String
    fields: Vec<(String, InnerType)>
    temporary: bool

BlockRange：整数范围，公开枚举
    All / Empty / Single(i64) / Range(Option<i64>, Option<i64>)
    map(self, length: usize, write: bool) -> Result<(usize, usize), BlockStorageError>
        将原范围映射为左闭右开区间，显式检查负端点、混号、逆序、越界及整数溢出；空Read[0]返回[0,0]。

BlockSelector：类型化选择器，公开结构体
    path: BlockPath
    range: BlockRange

BlockSource：事务求值来源，公开枚举
    Values(StoredBlockInner)--提交前已将alias/manual快照转成持久值
    Selector(BlockSelector)--仅worker在整次事务读取

BlockExpression：有序表达式，公开枚举
    Sources(Vec<BlockSource>)
    Empty--仅INJECT []按目标kind构造typed空值；GET不接受无类型Empty

BlockStorageOperation：机械操作，公开枚举
    Create(BlockSchema)
    Get { expressions: Vec<BlockExpression> }--同一read snapshot，保留参数边界
    Inject { source: BlockExpression, target: BlockSelector }--同一write事务求值、检查及replace

BlockStorageRequest：完整owned请求，公开结构体
    context: BlockStorageContext
    operation: BlockStorageOperation
    cancel: CancellationToken

StoredBlockResult：事务结果，公开枚举
    BlockOpened(BlockOpened)
    Arrays(Vec<StoredBlockInner>)--GET统一返回数组列表，单表达式由MCL取唯一项
    Unit--INJECT确认commit成功
    说明：机械契约不依赖MCL私有parser；Block/AgentBlock只可作为事务内或返回快照的纯值，不是Agent权威内存存储，不含RefBlock或RefMerge。

BlockStorageError：存储错误，公开枚举
    SchemaMismatch(String) / LifecycleMismatch(String) / BlockMissing(String) / FieldMissing(BlockPath)
    TypeMismatch / InvalidRange / InvalidRequest(String) / Cancelled
    CommitUnknown(String) / CleanupFailed(String) / Io(String)
    说明：Cancelled仅用于确认未提交；CommitUnknown不伪称回滚。CleanupFailed表示事务清理未获证明，Other活动不能按普通错误释放。

MclInstruction：指令内容，公开结构体
    text: String--原始指令文本
    arguments: Vec<serde_json::Value>--每项是一个数组，按问号出现顺序对应；无问号为空
    说明：单问号也不省略外层，数量和类型不符报错，不接受binding兼容层隐式猜测。

HistoryRecord：历史记录，公开共享契约
    key: String--完整Agent Entity、请求ID、类别和调用ID或index形成的幂等键
    source: String
    turn: Option<String>--执行开始时快照
    kind: String--mcl、assistant、tool_call及消息类别
    content: String--指令原文或消息正文
    payload: serde_json::Value--完整消息/usage/原指令及展开参数
    说明：相同key及相同内容重复追加为幂等成功，内容冲突返回错误；批量接口单事务全写或全不写。
```

## 逻辑

```text
共享组件：
    agent_components：Agent、AgentInfo、AgentState及WorldAgentExt
    lua_runtime_components：AgentLua、Driver通知与关闭槽位
    mcl_events：AgentMclQueue、MclPending、内部宿主提交及取消唤醒载荷、公开命令与结果
    execution_components：AgentExecutionScope及活动机制，Agent只引用一个作用域，不挂平行活动组件
    workspace_components：工作区身份、成员及WorldWorkspaceExt；workspace_execution提供线程入口与关闭能力
    resource_id_plugin：ResourceID、ResourceInner、ResourcesComponent、LibraryComponent、ResourceSet和基础查询
    types/resource_components：三类资源及AgentDriver，AgentDriver不强制有alias
    client_components：Client和其连接/发送器受检查询契约

资源规则：
    ResourceSet按完整ID存放目标Entity本地条目，可跨Entity有同ID，不能全局取第一个匹配者
    Agent内alias跨所有导入类型唯一，由ImportBindings及共享受检World入口统一维护；同alias不同完整ID先拒绝，Inference和MCL工具准备不能绕过检查
    IMPORT必须精确属于Agent驱动依赖，无builtin例外；MCL与加载插件共用WorldAgentExt.agent_validate_import受检预检，不维护平行许可或alias规则
    IMPORT TOOL tool:... AS alias产生CallableResource；IMPORT MESSAGE prompt:... AS alias按实际role产生AgentMessage（包括新System契约）；IMPORT SANDBOX sandbox:...产生SandboxValue并遵守同一alias检查
    重载成功更新资源条目及对应alias的新值，失败保留旧值；持久Block只存TOOL/SANDBOX ID，后续GET解析新定义，既有GET/execution快照不暗变；激活沙箱改变策略时遵守Sandbox活动互斥

值与输出：
    GET alias或selector lists始终返回MESSAGE/TOOL/SANDBOX typed数组；列表按顺序只拼接同类型，混合类型在空数组情况下也拒绝
    INJECT左侧与GET同一求值规则，右侧为唯一写入范围；handler不按目标字段类型转换输入值
    selector GET的全部存储读取在一次read transaction，完成后基于本次RAM绑定快照resolve；INJECT读取snapshot、map检查和replace在同一write transaction，持久TOOL/SANDBOX只写ID
    Agent.database保存一个Agent SQLite句柄，history是同文件分表视图；Block与history不同指令间没有跨事务原子保证
    默认CREATE BLOCK open/validate不清空，created只标初建；启动prompt/conversation与默认工具配置仅created=true时填充，不能每次启动覆盖
    CREATE TEMP BLOCK同instance幂等检查schema，TEMP entry不恢复；turn、pending、alias/import及active sandbox都不由数据库自动恢复
    MCL start_turn仅允许Idle，原子创建唯一turn并置Working；abort立即清空turn但保持Working；finish_turn核对当前Driver结果关联及本轮真实清理完成后清空turn并置Idle，Idle重复完成幂等，Working且turn=None不代表已完成；旧effect finish拒绝
    tool_call effect第一参数必须为恰好一个Assistant的MESSAGE数组，第二参数必须为TOOL数组；只执行该Assistant.tool_calls
    空tool_calls返回MESSAGE空数组；重复调用id、多个Assistant或工具alias冲突均在提交任何执行前失败，不产生部分submit
    输入Assistant不在tool_call effect中重复记录，输出为ToolCall结果消息组成的MESSAGE数组
    history_append允许范围独立保持inject/user/assistant/tool_call/error，System不通过此effect追加；infer支持System
    inference完整返回InferenceResponse，保留stop_reason，不只抽取assistant正文
    AbortAgentTurn控制系统在RuntimePlugin::PRE_UPDATE执行，先于UPDATE中的MCL实际执行及Inference/Tool受理；共享中止事件由各消费者独立观察，不依赖take reply独占通知；自定义调度保留该顺序
    实际执行infer/tool_call再次核对请求turn与当前turn；None或已中止旧轮次返回正常取消结果，不启动HTTP/runner
    工具用户轮次取消且cleanup成功返回Ok ToolCall：failed=true、content="Tool call aborted."，保留调用ID/名称及已知resource_id；未启动项同用该正文
    批次每个调用恰有一个原序结果；保留已完成真实结果，其余项（含未启动项）取消，不撤销已发生副作用
    CleanupFailed仍为Err并保留活动，不伪造cancelled或Idle；根Agent shutdown、owner abort、timeout、receiver closed保持独立原因
    finish_turn只结束正常无工具调用assistant或cancelled assistant的正常返回路径；真实Err不得伪造cancelled继续正常收口
    MclCommandResult.result是真实执行结果；message保存仅由显式history_append发起，history_error只用于附带指令审计错误，不能因此重做工具
```

# error

## 逻辑

```text
错误归对应共享领域契约，不引用业务插件私有类型。
执行前历史失败阻止执行，执行后历史失败不覆盖真实结果；history_append本身失败正常回错误。
Cancelled、CleanupFailed、重复请求、alias冲突和幂等历史内容冲突必须有明确分类。
From<AsyncTaskError>不能凭空取得请求ID，闭包提交包装必须保存关联，不能仅日志后丢失回执。
```
