# lib

## 类型

crate公开：
```text
MclEnvironmentProvider：MCL Lua环境提供器，crate公开结构体--由MclPlugin注册到LuaRuntimePlugin
    events: RuntimeEventSender--向MclPlugin发送MclCommandRequest，私有
    name(&self) -> &str
        获取名称：返回mcl
    provide(&self, context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError>
        提供MCL函数：注入mcl(target_agent_id, command, binding?)；每次调用都显式指定目标Agent，等待MclCommandReply后恢复同一Lua调用栈并返回值或Lua错误
```

公开：
```text
MclPlugin：Model Context Language领域运行时插件，公开结构体--解析MCL命令、执行领域操作并完成命令回执
    schedule: String--System所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开构建方法
    impl Default for MclPlugin
        Default：公开trait实现，调用new
    impl Plugin for MclPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：要求RuntimePlugin、LuaRuntimePlugin、ResourceIdPlugin、AgentImageLoaderPlugin、AgentPlugin、InferencePlugin、ToolPlugin和MemoryPlugin已安装
            行为：
                重复安装时panic
                插入MclPluginInstalled
                插入空PendingMclImports和PendingMclEffects
                注册MclCommandRequest、MclDomainRequest和MclDomainResponse事件
                在schedule依次挂载mcl_command_request_system、mcl_domain_system、mcl_import_response_system、mcl_effect_response_system和mcl_command_reply_system

MclPluginInstalled：MclPlugin安装标记，公开Resource
    impl Resource for MclPluginInstalled
```

crate公开：
```text
PendingMclImports：等待资源Provider响应的IMPORT事务，crate公开Resource
    imports: HashMap<String, MclImportState>--AgentResourceRegisterRequest.id到原IMPORT事务的唯一映射
    impl Resource for PendingMclImports

PendingMclEffects：等待外部结果的Effect事务，crate公开Resource
    effects: HashMap<String, MclEffectState>--以"mcl-effect:"加MclCommandId构造的请求ID到原Effect事务的唯一映射
    failures: HashMap<(Entity, String), MclError>--普通推理或工具链失败先于下一次start时，按Agent和turn_id暂存唯一失败
    impl Resource for PendingMclEffects
```

## 逻辑

```text
插件职责：
    MclPlugin解析MCL命令并执行Block、RefBlock和Effect领域操作
    MclPlugin按agent_id查找Agent Entity，通过Agent组件中的AgentMcl读写MCL数据
    MclPlugin将最终结果写入请求携带的一次性回执

插件边界：
    命令来源以及调用方收到回执后的传输不属于MclPlugin
    MclPlugin只通过LuaRuntimePlugin的通用VM邮箱请求实现start；不解释Lua源码，也不依赖WebSocket或具体前端
    MclPlugin不根据Block名称、字段名称、数组位置或请求来源推断上下文语义
    MclPlugin不挂载独立AgentMcl Component；AgentMcl只存在于Agent组件内部
```

# events

## 类型

公开：
```text
MclCommandRequest：MCL命令请求，公开事件--所有入口提交给MclPlugin的统一请求
    id: MclCommandId--进程内唯一命令ID
    agent_id: ResourceId--调用方显式指定的目标Agent完整资源ID
    command: String--命令文本
    binding: Option<serde_json::Value>--命令绑定值
    reply: MclCommandReply--调用方创建的一次性回执
    impl Event for MclCommandRequest
    impl Clone for MclCommandRequest
```

crate公开：
```text
MclDomainRequest：MCL领域请求，crate公开事件--命令解析System为IMPORT或EMIT EFFECT产生的跨领域操作
    id: MclCommandId--原命令ID
    agent_id: ResourceId--原请求目标Agent资源ID
    operation: MclOperation--已解析、尚未执行的领域操作
    reply: MclCommandReply--从命令请求原样传递的回执
    impl Event for MclDomainRequest
    impl Clone for MclDomainRequest

MclDomainResponse：MCL领域响应，crate公开事件--领域操作完成后产生的类型化结果
    id: MclCommandId--原命令ID
    agent_id: ResourceId--原请求目标Agent资源ID
    result: Result<MclDomainValue, MclError>--领域结果
    reply: MclCommandReply--从领域请求原样传递的回执
    impl Event for MclDomainResponse
    impl Clone for MclDomainResponse

MclImportState：IMPORT事务状态，crate公开结构体
    command_id: MclCommandId--原MCL命令ID
    agent_id: ResourceId--原命令目标Agent资源ID
    agent: Entity--清单验证时解析出的目标Agent Entity
    resource_id: ResourceId--待导入的完整资源ID
    alias: String--待登记的Agent内别名
    reply: MclCommandReply--原命令的一次性回执，只由响应链最终完成

MclEffectState：等待型Effect事务，crate公开结构体
    command_id: MclCommandId--原MCL命令ID
    agent_id: ResourceId--原命令目标Agent资源ID
    agent: Entity--开始Effect时解析出的目标Agent Entity
    reply: MclCommandReply--原命令的一次性回执，只由匹配响应最终完成
    kind: MclPendingEffectKind--用于验证响应类型及附加定位信息

MclPendingEffectKind：等待型Effect分类，crate公开枚举
    Start { vm_id: LuaVmId }
    CatchInference
    RealtimeLoad
    impl Clone for MclPendingEffectKind

MclExternalEffectResponse：等待型Effect的已接收外部响应，crate公开枚举--只用于统一移除pending和完成原回执
    Start(LuaVmMessageReceived)
    CatchInference(CapturedInferenceResponse)
    RealtimeLoad(AgentRealtimeContextReadCompleted)
    impl Clone for MclExternalEffectResponse
```

# types

## 类型

公开：
```text
MclMessage：MCL消息，公开结构体--携带领域消息及仅供MCL使用的本轮Token用量
    message: Message--领域消息
    usage: Option<TokenUsage>--本轮模型响应用量；非模型消息通常为空
    impl Clone for MclMessage
    Lua边界：转换为Lua表时平铺Message字段并追加可选usage，使Base Lua直接使用entry.type和entry.usage

BlockInner：Block内部值，公开枚举--Block字段持有的三种同质有序数组
    Message(Vec<MclMessage>)
    ToolCall(Vec<ToolCall>)
    ResourceId(Vec<ResourceId>)

InnerType：Block元素类型，公开枚举--命令解析和类型检查使用的无数据类型标记
    Message
    ToolCall
    ResourceId

BlockPath：Block字段路径，公开结构体--引用Block字段的逻辑地址，不是Rust借用
    block_id: String--BlockAssembly中的Block标识
    inner_id: String--Block中的字段标识

Block：Block，公开结构体--持有由MCL声明的字段
    inners: HashMap<String, BlockInner>--字段标识到同类型有序数组

BlockAssembly：Block程序集，公开结构体--持有一个MCL程序创建的所有真实Block
    blocks: HashMap<String, Block>--Block标识到Block

RefMerge：引用合并，公开枚举--按声明顺序保存一组同类型BlockPath
    Message(Vec<BlockPath>)
    ToolCall(Vec<BlockPath>)
    ResourceId(Vec<BlockPath>)
    paths(&self) -> &[BlockPath]
        读取引用路径：公开方法，不读取或复制目标数组
    iter(&self, blocks: &BlockAssembly) -> Result<BlockInner, MclError>
        迭代引用合并：公开方法，按声明顺序解析路径、验证同类型并克隆拼接元素

RefBlock：引用Block，公开结构体--持有MCL声明的引用合并
    merges: HashMap<String, RefMerge>--merge标识到引用合并

RefBlockAssembly：引用Block程序集，公开结构体--持有所有MCL声明的引用Block
    blocks: HashMap<String, RefBlock>--引用Block标识到引用Block

MclDeleteSelection：字段机械删除范围，公开枚举--MclPlugin解析DELETE语义后交给AgentMcl.delete
    All--删除全部元素
    First--删除首个元素
    Indices(Vec<usize>)--删除指定位置；索引已去重并按降序排列

MclCommandId：MCL命令ID，公开新类型--由Margatroid入口生成的进程内唯一命令标识
    0: String
    new(value: impl Into<String>) -> Result<Self, MclError>
        构造命令ID：公开关联函数，拒绝空字符串
    as_str(&self) -> &str
        读取命令ID：公开方法
    impl Clone for MclCommandId

MclCommandReply：MCL命令回执，公开结构体--所有命令入口统一使用的一次性回执
    sender: Arc<Mutex<Option<oneshot::Sender<Result<MclCommandValue, MclError>>>>>--私有发送器槽位
    new(sender: oneshot::Sender<Result<MclCommandValue, MclError>>) -> Self
        构造回执：公开关联函数
    send(&self, result: Result<MclCommandValue, MclError>)
        发送结果：crate公开方法，最多发送一次；接收端已取消时丢弃结果
    impl Clone for MclCommandReply
```

纯数据类型归属：
```text
BlockInner、BlockPath、Block、BlockAssembly、RefMerge、RefBlock、RefBlockAssembly、MclDeleteSelection和MclRealtimeSource实际定义在共享types crate
MclPlugin重新导出这些MCL数据类型；ResourceId由ResourceIdPlugin提供
```

crate公开：
```text
BlockFieldDeclaration：CREATE BLOCK字段声明，crate公开枚举
    Empty { inner_id: String, inner_type: InnerType }--创建指定类型的空字段
    Merge { inner_id: String, sources: Vec<BlockPath> }--先合并来源，再用结果初始化字段
    impl Clone for BlockFieldDeclaration

RefMergeDeclaration：CREATE REF_BLOCK引用字段声明，crate公开结构体
    merge_id: String--新RefBlock中的字段标识
    sources: Vec<BlockPath>--只允许指向真实Block字段的有序路径
    impl Clone for RefMergeDeclaration

MclBinding：已经从MclCommandRequest.binding取得的绑定值，crate公开新类型
    0: serde_json::Value
    impl Clone for MclBinding

MclPredicate：DELETE WHERE已经解析的删除条件，crate公开枚举
    ToolCallIdEquals(String)--删除ToolCall.id等于绑定字符串的元素；其他InnerType拒绝
    impl Clone for MclPredicate

MclOperation：MCL命令解析后的领域操作，crate公开枚举
    CreateBlock { block_id: String, fields: Vec<BlockFieldDeclaration> }
    CreateRefBlock { block_id: String, merges: Vec<RefMergeDeclaration> }
    Merge { sources: Vec<BlockPath> }
    RefMerge { sources: Vec<BlockPath> }
    Import { resource_id: ResourceId, alias: String }
    Inject { target: BlockPath, value: MclBinding }
    InjectMany { target: BlockPath, values: Vec<MclBinding> }
    CoverValue { target: BlockPath, value: MclBinding }
    CoverInner { source: BlockPath, target: BlockPath }
    Select { source: BlockPath }
    DeleteAll { target: BlockPath }
    DeleteFirst { target: BlockPath }
    DeleteWhere { target: BlockPath, predicate: MclPredicate }
    Emit { effect: MclEffectCommand }
    impl Clone for MclOperation

MclDomainValue：MCL领域值，crate公开枚举--保留领域类型的操作结果
    Unit
    Inner(BlockInner)
    Paths(Vec<BlockPath>)
    Message(MclMessage)
    ResourceImport(ResourceImportReceipt)
    Text(String)
    impl Clone for MclDomainValue

MclCommandValue：MCL命令返回值，公开枚举--直接命令或领域响应最终交给调用方的值
    Unit
    Inner(BlockInner)
    Paths(Vec<BlockPath>)
    Message(MclMessage)
    ResourceImport(ResourceImportReceipt)
    Text(String)
    impl Clone for MclCommandValue

MclEffectCommand：MCL宿主Effect的已解析命令，crate公开枚举--保存语法参数，不读取AgentMcl
    Start
    CatchInference { ref_block_id: String }
    Inference { ref_block_id: String }
    ToolCall { calls: Vec<ToolCall> }
    Finish
    HistoryAppend { message: MclMessage }
    RealtimeSource { ref_block_id: String }
    RealtimeLoad
    impl Clone for MclEffectCommand

MclEffect：准备完成的MCL宿主Effect，crate公开枚举--名称与Base Lua公开Effect一一对应，只携带目标Agent中已经验证并展开的数据
    Start
    CatchInference { messages: Vec<MclMessage> }
    Inference { messages: Vec<MclMessage>, visible_resources: Vec<ResourceId> }
    ToolCall { calls: Vec<ToolCall> }
    Finish
    HistoryAppend { message: MclMessage }
    RealtimeSource { source: MclRealtimeSource, values: Vec<MclMessage> }
    RealtimeLoad
    impl Clone for MclEffect

MclRealtimeSource：实时上下文来源，crate公开结构体--保存动态引用而非声明时快照
    ref_block_id: String--realtime_source参数指定的RefBlock
    message_merge_id: String--该RefBlock中唯一的Message类型RefMerge
    dependencies: Vec<BlockPath>--Message RefMerge按声明顺序持有的真实Block字段路径
    impl Clone for MclRealtimeSource

MclEffectRequest：已展开的RefBlock Effect参数，crate公开结构体
    messages: Vec<MclMessage>--唯一Message RefMerge的当前有序快照
    visible_resources: Vec<ResourceId>--可选ResourceId RefMerge的当前有序快照，不存在时为空
    message_source: MclRealtimeSource--消息RefMerge的动态来源描述，只有realtime_source会保存

ResourceImportReceipt：资源导入回执，crate公开结构体
    resource_id: ResourceId
    alias: String
    available: bool
    error: Option<String>
    impl Clone for ResourceImportReceipt
```

## 命令

公开语法：
```text
CREATE BLOCK <block_id> (
    <inner_id> MESSAGE | TOOL_CALL | TOOL,
    MERGE <inner_id>, ... FROM <block_id> AS <inner_id>,
    ...
)
    创建真实Block；普通字段为空，MERGE字段以合并后的真实数组预填充。
    CREATE内部禁止SELECT；MERGE必须解析为真实数组后才能写入字段。

CREATE REF_BLOCK <ref_block_id> (
    REF_MERGE <inner_id>, ... FROM <block_id> AS <merge_id>,
    ...
)
    创建引用Block；RefBlock内部只能声明REF_MERGE字段，REF_MERGE只登记路径，不复制元素。
    引用源必须是真实Block字段，不能从RefBlock再次建立引用。

MERGE <inner_id>, ... FROM <block_id>
    对每个源字段调用AgentMcl.select，得到按源顺序排列的数组集合，再按声明顺序验证同类型并拍平合并为一个真实数组。
    源字段可以来自真实Block或RefBlock；因此MERGE是SELECT的批量拍平形式。
    只选择一个字段时等价于该字段的SELECT结果；不产生别名。

REF_MERGE <inner_id>, ... FROM <block_id>
    直接生成多个真实Block字段的路径数组，例如：
    REF_MERGE system, compact_context, history_conversation, recent_conversation FROM msg
    返回 [msg.system, msg.compact_context, msg.history_conversation, msg.recent_conversation]。
    它不读取字段内容，也不复制数组。

IMPORT <resource_id> AS <alias>
    当前Agent必须通过ResourceId找到Agent Entity；读取Agent.info.image_entity，
    再读取AgentImage Entity上的AgentImageDependencies。
    resource_id不在依赖清单时立即失败，不产生注册请求，也不登记alias。
    依赖存在时向资源注册协议发送验证请求；验证资源真实存在且Provider测试通过后，
    才将候选ResourceMapEntry写入Agent.resources并登记alias。
    验证失败时IMPORT失败，不写入部分映射；错误通过原MclCommandReply返回调用点。
    prompt:system/soul和prompt:user/compact都是MCL管理的prompt资源，不由AgentImage承载。
    MCL在IMPORT时直接读取Agent.info.image_root下的SOUL.md和COMPACT.md，
    成功时构造ResourceContent::Prompt并完成原回执；不写入AgentImage或AgentInfo的长期语义字段。

INJECT <binding> TO <inner_id> FROM <block_id>
    按目标字段类型解析binding并追加一个元素

INJECT <binding>, <binding>, ... TO <inner_id> FROM <block_id>
    按目标字段类型逐个解析多个binding并按顺序追加；不允许携带运行时binding参数

INJECT <binding> COVER <inner_id> FROM <block_id>
    按目标字段类型解析binding并用单元素数组覆盖字段

INJECT SELECT <source_inner> COVER <target_inner> FROM <block_id>
    验证源和目标同类型，将源数组克隆后覆盖目标字段

SELECT <inner_or_merge_id> FROM <block_or_ref_block_id>
    独立查询一个字段或引用合并字段；真实Block返回字段真实数组的克隆，
    RefBlock返回其引用路径迭代后的真实数组。SELECT不得作为CREATE的子表达式。

DELETE <inner_id> FROM <block_id>
    清空字段

DELETE <inner_id> FIRST FROM <block_id>
    删除首项；字段为空时返回错误

DELETE <inner_id> FROM <block_id> WHERE id == <binding>
    binding必须是字符串，目标字段必须是TOOL_CALL；删除全部id等于binding的ToolCall
    没有匹配项时幂等成功

EMIT EFFECT inference (<ref_block_id>)
    展开RefBlock中的消息与可见资源，发起普通推理；响应通过AgentMessage进入AgentPlugin和Lua VM邮箱

EMIT EFFECT start
    等待并读取Agent长期Lua VM邮箱中的下一条AgentMessage；无消息时保持命令pending

EMIT EFFECT catch_inference (<ref_block_id>)
    展开RefBlock中的消息，发起捕获式推理并等待文本结果；响应不生成AgentMessage

EMIT EFFECT tool_call ?
    把binding解析为ToolCall数组并发起工具调用；工具响应通过AgentMessage进入AgentPlugin和Lua VM邮箱

EMIT EFFECT finish
    结束当前Agent轮次并返回等待下一条AgentMessage的循环

EMIT EFFECT history_append ?
    显式追加历史消息

EMIT EFFECT realtime_source (<ref_block_id>)
    以RefBlock中的消息引用声明实时上下文来源；声明时以及其引用字段以后每次变更时都用完整当前内容覆盖实时上下文表

EMIT EFFECT realtime_load
    读取实时上下文快照
```

语法约束：
```text
AS只属于IMPORT以及CREATE内部的MERGE和REF_MERGE：
    IMPORT的AS登记资源别名；CREATE的AS是新Block/RefBlock中字段哈希表键。
顶层MERGE和REF_MERGE不接受AS，直接返回值。
CREATE内部不得使用SELECT；SELECT不能作为字段初始化器。
MERGE返回真实数组，REF_MERGE返回路径数组；只有SELECT命中RefBlock时才执行引用迭代。
MERGE通过AgentMcl.select读取每个源字段，因此允许源字段来自真实Block或RefBlock。
REF_MERGE的每个路径必须指向真实Block字段；任何RefBlock字段都不得作为REF_MERGE源。
CREATE BLOCK只能使用空字段或MERGE初始化字段；CREATE REF_BLOCK只能使用REF_MERGE初始化字段。
同一个CREATE中的所有MERGE/REF_MERGE先解析并校验，全部成功后原子创建，不保留半成品。
    SELECT不区分真实Block和RefBlock；类型判断与RefMerge迭代统一由AgentMcl.select完成。
    当一个操作选择多个字段时，SELECT阶段的结果是数组集合；MERGE才负责将其拍平为一个数组。
IMPORT必须先通过Agent镜像依赖清单检查，再通过资源Provider的存在性和可用性测试；
    任一步失败都返回错误，不静默创建别名或ResourceMapEntry。
Effect参数约束：
    inference、catch_inference和realtime_source的括号中必须恰好包含一个RefBlock ID，不接受隐式req/com名称或嵌套SELECT。
    RefBlock ID只是MCL源码定义的普通名称；宿主按InnerType展开，不按req、com、ctx或vis等名称推断用途。
    inference要求RefBlock恰有一个Message RefMerge，并允许至多一个ResourceId RefMerge；按各RefMerge内部路径顺序分别展开为messages和visible_resources。
    catch_inference要求RefBlock恰有一个Message RefMerge且没有ResourceId或ToolCall RefMerge，保证捕获响应只能产生文本。
    realtime_source要求RefBlock恰有一个Message RefMerge；其他类型RefMerge不属于实时表并被忽略，Message来源路径作为动态依赖保存。
    tool_call必须恰好使用一个binding；binding必须是非空ToolCall数组，调用ID在数组内唯一。
    start、finish和realtime_load不接受参数或binding。
    history_append保持既有单个MclMessage binding约定，不属于本轮七项Effect的请求编排。
每条非Effect命令只允许两种完成路径：
    CREATE、MERGE、REF_MERGE、INJECT、SELECT和DELETE在mcl_command_request_system内直接完成原MclCommandReply；
    IMPORT保存原MclCommandReply并等待AgentResourceRegisterResponse，响应到达后通过MclDomainResponse完成同一回执。
```

词法与绑定规范：
```text
命令按UTF-8文本解析，关键字、Block ID、inner ID、merge ID和alias全部大小写敏感；命令两端及关键字之间允许ASCII空白，换行等同空白，单条命令不接受分号后的第二条命令。
标识符统一使用[A-Za-z_][A-Za-z0-9_.-]*，拒绝空值、.、..、控制字符、反斜杠、冒号和路径分隔符；ResourceId仍由ResourceId::parse独立解析。
CREATE BLOCK和CREATE REF_BLOCK的括号内允许逗号分隔声明和一个可选尾逗号；重复inner_id或merge_id在解析阶段失败。
括号中的RefBlock ID必须是单个标识符；tool_call和history_append必须恰好使用一个binding，其余命令携带binding时返回InvalidCommand。
binding来自JSON值：MESSAGE必须是带type字段的Message对象；TOOL_CALL必须是非空数组且每项id、tool_name和arguments合法；TOOL必须是规范化ResourceId字符串；DELETE WHERE必须是非空字符串。
解析器返回的错误必须包含MclError分类和命令中的token位置；解析失败不得访问World或修改AgentMcl。
```

Lua回值规范：
```text
MclCommandValue::Unit -> nil
MclCommandValue::Inner(BlockInner::Message(values)) -> Message对象数组
MclCommandValue::Inner(BlockInner::ToolCall(values)) -> ToolCall对象数组
MclCommandValue::Inner(BlockInner::ResourceId(values)) -> 完整ResourceId字符串数组
MclCommandValue::Paths(paths) -> [{ block_id = string, inner_id = string }, ...]
MclCommandValue::Message(message) -> 平铺Message对象，type/content/reasoning/tool_calls/usage同MclMessage转换规则
MclCommandValue::ResourceImport(receipt) -> { resource_id, alias, available, error }
MclCommandValue::Text(value) -> Lua字符串
Rust错误统一转换为Lua错误；Lua端只能得到稳定错误分类和有界描述，不暴露World、SQL、绝对路径或Provider原始正文。
```

# system

## 函数

crate公开：
```text
mcl_command_request_system(world: &mut World)
    命令请求System：crate公开System
    处理事件：MclCommandRequest
    行为：克隆本帧全部MclCommandRequest并逐个调用handle_mcl_command_request
    命令解析失败：直接向request.reply发送Err，不发送领域事件
        CREATE、MERGE、REF_MERGE、INJECT、SELECT和DELETE：解析后直接调用对应execute_*函数，并把Result转换为MclCommandValue写入request.reply
    IMPORT和EMIT EFFECT：解析后发送MclDomainRequest，不直接执行跨插件操作
        本System不等待异步领域结果

mcl_domain_system(world: &mut World)
    领域执行System：crate公开System
    处理事件：MclDomainRequest
    行为：
        克隆本帧全部MclDomainRequest并逐个调用handle_mcl_domain_request
        只接受Import和Emit操作；收到本地Block操作时发送EffectInvalid响应
        跨领域提交完成或结果到达后发送MclDomainResponse
        等待型操作保存id、agent_id和reply，完成后再发送MclDomainResponse

mcl_import_response_system(world: &mut World)
    IMPORT响应System：crate公开System
    处理事件：AgentResourceRegisterResponse
    行为：克隆本帧全部响应并逐个调用handle_import_response
        按response.id从PendingMclImports移除唯一MclImportState
        找到事务时调用commit_import，并把结果包装成保留原command_id、agent_id和reply的MclDomainResponse
        未找到事务表示该响应不属于当前MCL IMPORT，直接跳过，不调用commit_import且不产生MclDomainResponse

mcl_effect_response_system(world: &mut World)
    Effect响应System：crate公开System
    处理事件：LuaVmMessageReceived、CapturedInferenceResponse、AgentRealtimeContextReadCompleted、AgentFailure和LuaRuntimeTaskFinished
    行为：按事件ID从PendingMclEffects移除唯一MclEffectState，验证响应类型、Agent和VM定位后调用对应handle_*_response
        成功和失败都产生恰好一个保留原command_id、agent_id和reply的MclDomainResponse
        未找到事务表示响应不属于当前MCL Effect或已经处理，直接跳过且不重复完成回执
        AgentFailure按agent和id匹配正在等待的Start；已存在Start时以Err完成该回执，否则写入failures供下一次Start先行取得
        LuaRuntimeTaskFinished按owner_id定位Agent并调用handle_agent_vm_finished，关闭该Agent尚未完成的全部MCL事务

mcl_command_reply_system(world: &mut World)
    命令回执System：crate公开System
    处理事件：MclDomainResponse
    行为：克隆本帧全部MclDomainResponse并逐个调用handle_mcl_domain_response
```

# error

## 类型

公开：
```text
MclError：MCL错误，公开枚举--描述命令解析、Agent寻址、Block寻址、类型和Effect错误
    ParseFailed
    InvalidAgentId
    AgentMissing
    DuplicateAgent
    AgentRuntimeMissing
    BlockMissing { assembly: String, block: String }
    InnerMissing { block: String, inner: String }
    RefBlockMissing { assembly: String, block: String }
    MergeMissing { block: String, merge: String }
    TypeMismatch
    BindingMissing
    InvalidCommand
    ImportMissing
    ImportFailed
    ImportResponseMismatch
    MessageSourceUnavailable
    EffectAlreadyPending
    EffectResponseMismatch
    TurnMissing
    TurnMismatch
    MailboxFailed
    InferenceFailed
    ToolCallInvalid
    RealtimeReadFailed
    EffectInvalid
    SourceReadFailed
    impl fmt::Display for MclError
        Display：公开trait实现，输出稳定错误类型及寻址上下文
    impl std::error::Error for MclError
    impl Clone for MclError
```

# handler

## 函数

crate公开：
```text
handle_mcl_command_request(world: &mut World, request: MclCommandRequest)
    处理命令请求：crate公开函数
    行为：
        验证request.agent_id是type=agent的完整ResourceId；验证失败立即调用request.reply.send(Err(error))并返回
        调用parse_operation(&request.command, request.binding.as_ref())解析为MclOperation
        解析失败时直接调用request.reply.send(Err(error))并返回
        CREATE、MERGE、REF_MERGE、INJECT、SELECT和DELETE调用execute_direct_operation
        直接操作成功或失败都调用request.reply.send，绝不发送MclDomainRequest
        IMPORT和EMIT EFFECT构造保留相同id、agent_id、reply的MclDomainRequest

handle_mcl_domain_request(world: &mut World, request: MclDomainRequest)
    执行领域请求：crate公开函数
    行为：
        Import调用begin_import；开始成功后不发送MclDomainResponse，由mcl_import_response_system等待Provider响应
        begin_import同步失败时由自身立即发送保留原id、agent_id和reply的MclDomainResponse
        Emit先调用prepare_effect按目标Agent的AgentMcl验证并展开参数，再路由到唯一Effect处理函数
        prepare_effect失败时立即发送保留原id、agent_id和reply的Err MclDomainResponse，不登记pending、不发送下游事件
        不在路由层解释Block名称或Effect参数位置
        提交即完成的Effect发送保留相同id、agent_id、reply的MclDomainResponse
        等待型操作由所属领域保存原reply并在完成后发送MclDomainResponse
        收到CREATE、MERGE、REF_MERGE、INJECT、SELECT或DELETE时发送EffectInvalid响应

prepare_effect(world: &World, agent_id: &ResourceId, command: MclEffectCommand) -> Result<MclEffect, MclError>
    准备Effect：crate公开函数
    行为：
        Start、Finish和RealtimeLoad直接转换为同名MclEffect
        ToolCall和HistoryAppend使用解析阶段已经验证的binding值
        Inference调用resolve_effect_request并保留messages和visible_resources
        CatchInference调用resolve_effect_request，要求visible_resources为空，只保留messages
        RealtimeSource调用resolve_effect_request，保留message_source和messages当前完整快照，忽略不属于实时表的visible_resources
        任一来源不存在、RefMerge类型数量不合法或迭代失败都返回Err，且不调用任何Effect处理函数

handle_start_effect(world: &mut World, request: MclDomainRequest)
    等待下一条消息：crate公开函数
    行为：
        按request.agent_id找到唯一Agent，要求生命周期为Creating或Running且长期Lua VM存在
        Agent已有当前turn且PendingMclEffects.failures存在相同(agent, turn_id)时，取出失败、abort当前turn并立即发送Err的MclDomainResponse，不读取下一条邮箱消息
        构造"mcl-effect:" + request.id并拒绝PendingMclEffects中的重复ID
        先保存MclEffectState { kind: Start { vm_id }, ... }，再发送LuaVmMessageReceiveRequest { id, vm_id }
        VM邮箱已有值时LuaRuntimePlugin立即取出最早一项；为空时由LuaRuntimePlugin保存唯一receive请求，后续消息到达再响应
        Agent仍为Creating且initialization.complete=false时，在receive请求成功登记后发布AgentInitializationCompleted { agent, vm_id }；这是Base Lua进入消息循环的唯一完成信号
        请求提交成功不完成原reply；提交失败时移除pending并立即发送Err的MclDomainResponse

handle_start_response(world: &mut World, state: MclEffectState, response: LuaVmMessageReceived) -> Result<MclDomainValue, MclError>
    完成start：crate公开函数
    行为：
        验证state.kind、response.id和vm_id完全匹配，将response.result错误转换为MailboxFailed
        将邮箱值解析为内部信封 { turn_id, message: MclMessage }；信封不直接暴露给Lua
        User消息要求Agent.turn为空并调用begin(turn_id)；Assistant和Tool消息要求Agent.turn.turn_id等于turn_id
        成功返回MclDomainValue::Message(message)，Lua中的handle只取得MclMessage
        类型、轮次或信封验证失败返回Err；该响应已消费，不重放邮箱消息

handle_catch_inference_effect(world: &mut World, request: MclDomainRequest, messages: Vec<MclMessage>)
    发起捕获式推理：crate公开函数
    行为：
        找到唯一Agent并验证存在当前turn；将MclMessage依次脱壳为Message
        使用"mcl-effect:" + request.id作为捕获请求ID，先保存kind=CatchInference的MclEffectState
        发送CapturedInferenceRequest { id, agent, agent_id, messages }；请求固定tools=[]且不配置普通Agent流式消息发送器
        提交成功不完成原reply；提交失败时移除pending并立即发送Err的MclDomainResponse

handle_catch_inference_response(world: &mut World, state: MclEffectState, response: CapturedInferenceResponse) -> Result<MclDomainValue, MclError>
    完成捕获式推理：crate公开函数
    行为：验证state.kind、响应ID和Agent，错误转换为InferenceFailed；成功正文必须非空，返回MclDomainValue::Text
        不构造AgentMessage，不调用AgentPlugin，不写历史，不进入工具循环

handle_inference_effect(world: &mut World, request: MclDomainRequest, messages: Vec<MclMessage>, visible_resources: Vec<ResourceId>)
    发起普通推理：crate公开函数
    行为：
        找到唯一Agent并取得当前turn_id；不存在当前轮次返回TurnMissing
        将MclMessage按原顺序脱壳为Message，通过ToolPlugin的resolve_agent_tool_definitions按visible_resources顺序生成本次ToolDefinition快照
        全部验证完成后发送InferenceRequestEvent { id: turn_id, agent, agent_id, messages, tools }
        事件提交成功立即发送MclDomainValue::Unit完成原reply，不等待模型响应
        同步验证或提交失败立即通过同一MclDomainResponse返回Err，不产生部分推理请求
        模型成功响应由InferencePlugin构造AgentMessage，经AgentPlugin进入VM邮箱，供下一次start取得

handle_tool_call_effect(world: &mut World, request: MclDomainRequest, calls: Vec<ToolCall>)
    发起工具调用：crate公开函数
    行为：
        找到唯一Agent并取得当前turn_id；不存在当前轮次返回TurnMissing
        在发送任何事件前验证calls非空、调用ID非空且互不重复，并调用ToolPlugin验证每个tool_name在Agent.resources中存在且可执行
        验证全部成功后按calls顺序逐个发送ToolCallEvent { turn_id, agent, call }
        提交成功立即发送MclDomainValue::Unit完成原reply，不等待工具结果
        工具成功或执行错误都由ToolPlugin构造Message::Tool的AgentMessage，经AgentPlugin进入VM邮箱，供后续start逐条取得

handle_finish_effect(world: &mut World, request: MclDomainRequest)
    结束工具调用循环：crate公开函数
    行为：找到唯一Agent，要求Agent.tools.pending中没有当前轮次调用，读取当前turn_id并调用Agent.turn.finish；成功立即以MclDomainValue::Unit完成原reply
        没有当前轮次或轮次不一致时返回Err，不发送推理、工具或AgentMessage事件
        finish只结束当前轮次；Base Lua继续while循环并在下一次start等待新消息

handle_realtime_load_effect(world: &mut World, request: MclDomainRequest)
    加载实时上下文表：crate公开函数
    行为：
        找到唯一Agent，构造"mcl-effect:" + request.id，先保存kind=RealtimeLoad的MclEffectState
        发送AgentRealtimeContextReadRequested { id, agent }；提交成功不完成原reply
        提交失败时移除pending并立即发送Err的MclDomainResponse

handle_realtime_load_response(world: &mut World, state: MclEffectState, response: AgentRealtimeContextReadCompleted) -> Result<MclDomainValue, MclError>
    完成实时加载：crate公开函数
    行为：验证state.kind、响应ID和Agent；读取错误转换为RealtimeReadFailed
        成功把有序Vec<MclMessage>包装为MclDomainValue::Inner(BlockInner::Message)，恢复原Lua调用点

handle_realtime_source_effect(world: &mut World, request: MclDomainRequest, source: MclRealtimeSource, values: Vec<MclMessage>)
    定义实时上下文来源：crate公开函数
    行为：
        找到唯一Agent；再次验证source.ref_block_id、message_merge_id和dependencies仍对应同一个Message RefMerge
        只有来源验证和values完整展开都成功后，才用source整体替换Agent.mcl.realtime_source
        随即发送AgentRealtimeContextWriteRequested { agent, messages: values }，使声明当刻的完整内容覆盖实时上下文表
        事件提交后立即以MclDomainValue::Unit完成原reply；Memory持久化失败通过AgentMemoryWriteFailed报告，不回滚已经成立的MCL来源声明

handle_history_append_effect(world: &mut World, request: MclDomainRequest, message: MclMessage)
    追加历史消息：crate公开函数
    行为：
        找到唯一Agent并取得当前turn_id；没有当前轮次或request.id与当前轮次不匹配时返回TurnMissing或TurnMismatch
        将MclMessage脱壳为Message，拒绝System并按Message角色构造AgentHistoryMessageWriteRequested
        User和Tool的tool_schema、usage固定为空；Assistant的tool_schema从Agent.inference.pending中按turn_id取得并在发送事件后清除
        发送事件成功后立即以MclDomainValue::Unit完成原reply；Memory写入是独立持久化事务，不阻塞Lua调用点
        MemoryPlugin后续发送AgentMemoryWriteFailed时由mcl_memory_failure_system记录稳定错误并设置Agent.last_error；不重复完成已发送的MCL回执

mcl_memory_failure_system(world: &mut World)
    处理历史或实时写入失败：私有System，读取AgentMemoryWriteFailed
    行为：按Agent定位并写入Agent.last_error；若错误对应当前Creating或Running Agent，不伪造AgentMessage、不重试、不回滚已经完成的MCL命令

handle_effect_response(world: &mut World, response_id: &str, response: MclExternalEffectResponse)
    整理等待型Effect响应：crate公开函数
    行为：按response_id从PendingMclEffects原子移除事务，根据state.kind只接受对应的Lua邮箱、捕获推理或实时读取响应
        调用对应handle_*_response后，无论成功失败都发送恰好一个MclDomainResponse
        类型不符返回EffectResponseMismatch；事务已移除，重复或迟到响应不能再次完成原reply

handle_agent_failure(world: &mut World, failure: AgentFailure)
    关闭普通异步失败：crate公开函数
    行为：
        查找同一Agent且kind=Start的pending，并读取Agent当前turn_id验证等于failure.id
        已有等待者时移除该Start、调用Agent.turn.abort，并按failure.kind把Inference、Tool、Agent分别转换为InferenceFailed、ToolCallInvalid、EffectInvalid完成原MclDomainResponse
        尚未进入下一次start时，把错误写入PendingMclEffects.failures[(agent, failure.id)]；同键重复失败保留第一项
        失败不会伪造成User、Assistant或Tool消息，也不会永远留下一个无法完成的start回执

handle_agent_vm_finished(world: &mut World, event: LuaRuntimeTaskFinished)
    清理已结束VM的MCL事务：crate公开函数
    行为：
        从event.owner.owner_id解析Agent ResourceId并定位Entity；只处理该Agent对应的长期Base Lua VM
        从PendingMclEffects移除该Agent全部Start、CatchInference和RealtimeLoad，为每项发送MailboxFailed的MclDomainResponse
        CatchInference同时发送CancelInferenceRequest { id: pending请求ID, agent }；RealtimeLoad的迟到响应因pending已移除而丢弃
        从PendingMclImports移除该Agent全部IMPORT并以ImportFailed完成原MclDomainResponse，迟到Provider响应不再提交资源映射
        清空该Agent的failures；若存在当前turn则发送CancelInferenceRequest和CancelToolTurn后调用Agent.turn.abort
        每个被移除事务恰好完成一次原reply，VM停止不会留下永久pending命令

begin_import(world: &mut World, request: MclDomainRequest, resource_id: ResourceId, alias: String)
    开始IMPORT事务：crate公开函数
    行为：
        调用validate_import_dependency取得目标Agent Entity
        使用"mcl-import:" + request.id.as_str()生成带命名空间的AgentResourceRegisterRequest.id；PendingMclImports已有同ID时产生InvalidCommand
        构造包含原command_id、agent_id、agent、resource_id、alias和reply的MclImportState
        先把事务写入PendingMclImports，再发送AgentResourceRegisterRequest { id, agent, resource_id, alias: Some(alias) }
        成功只表示注册请求已提交，不完成MclCommandReply，也不发送MclDomainResponse
        清单验证或Pending写入失败时不发送注册请求，直接发送保留request.id、request.agent_id和request.reply的MclDomainResponse

validate_import_dependency(world: &World, agent_id: &ResourceId, resource_id: &ResourceId) -> Result<Entity, MclError>
    IMPORT清单验证：
        通过WorldResourceIdExt::entity_by_resource_id找到Agent Entity
        读取Agent组件的AgentInfo.image_entity
        读取image_entity上的AgentImageDependencies组件
        按完整ResourceId精确查找依赖项；找不到返回ImportMissing
        找到后返回目标Agent Entity
        不读取来源source执行下载；source只作为依赖元数据传给Provider
        Provider协议保证每个AgentResourceRegisterRequest最终恰好产生一个AgentResourceRegisterResponse

handle_import_response(world: &mut World, response: AgentResourceRegisterResponse)
    处理IMPORT响应：crate公开函数
    行为：
        按response.id从PendingMclImports原子移除事务；不存在时直接返回，因为该共享响应不属于当前MCL IMPORT
        调用commit_import(world, &state, response)
        无论提交成功或失败，都发送恰好一个保留state.command_id、state.agent_id和state.reply的MclDomainResponse
        事务已经移除，重复响应不会再次完成原回执

commit_import(world: &mut World, state: &MclImportState, response: AgentResourceRegisterResponse) -> Result<MclDomainValue, MclError>
    IMPORT提交：
        验证响应id等于"mcl-import:" + state.command_id、agent等于state.agent、resource_id等于state.resource_id且alias等于Some(state.alias)；不一致返回ImportResponseMismatch
        response.result为Err时转换为ImportFailed
        验证成功响应中的ResourceMapEntry由Provider确认真实存在且测试通过，并与state.resource_id一致
        调用ToolPlugin::register_agent_resource写入目标Agent.resources
        ResourceMap写入成功后返回MclDomainValue::ResourceImport，available固定为true且error固定为None
        任何失败都不写入别名或映射；错误通过原MclCommandReply返回Lua调用点，不发送AgentPlugin专用IMPORT事件

handle_mcl_domain_response(world: &mut World, response: MclDomainResponse)
    完成命令回执：crate公开函数
        行为：把Unit、Inner、Paths、Message、ResourceImport和Text逐项转换为同名MclCommandValue，并恰好调用一次response.reply.send
        Err原样交给response.reply.send；不发送响应事件、不重试、不缓存

requires_domain(operation: &MclOperation) -> bool
    判断领域事件：Import和Emit返回true；CreateBlock、CreateRefBlock、Merge、RefMerge、Inject、CoverValue、CoverInner、Select、DeleteAll、DeleteFirst、DeleteWhere返回false
```

crate公开：
```text
execute_direct_operation(world: &mut World, request: &MclCommandRequest, operation: MclOperation) -> Result<MclCommandValue, MclError>
    直接命令执行：
        operation为CreateBlock时调用parse_block_fields构造完整Block，再调用AgentMcl.create_block，成功返回MclCommandValue::Unit
        operation为CreateRefBlock时调用parse_ref_merges构造完整RefBlock，再调用AgentMcl.create_ref_block，成功返回MclCommandValue::Unit
        operation为Merge时调用execute_merge并返回MclCommandValue::Inner
        operation为RefMerge时调用execute_ref_merge并返回MclCommandValue::Paths
        operation为Inject时调用execute_inject_to并返回MclCommandValue::Unit
        operation为CoverValue时调用execute_inject_cover并返回MclCommandValue::Unit
        operation为CoverInner时调用execute_inject_select_cover并返回MclCommandValue::Unit
        operation为Select时调用execute_select_from并返回MclCommandValue::Inner
        operation为DeleteAll、DeleteFirst或DeleteWhere时调用execute_delete并返回MclCommandValue::Unit
        operation为Import或Emit时返回EffectInvalid；这些操作必须走MclDomainRequest
        每个分支只返回Result，不自行发送回执或领域事件；handle_mcl_command_request恰好调用一次request.reply.send

parse_block_fields(agent_mcl: &AgentMcl, declarations: &[BlockFieldDeclaration]) -> Result<Block, MclError>
    CREATE BLOCK初始化解析：
        普通字段声明只登记类型并创建空数组
        MERGE声明调用AgentMcl.merge，得到同类型真实数组作为字段初值；其源可由select读取真实Block或RefBlock
        拒绝重复inner_id；所有字段先写入临时Block，任一字段失败时不创建Block
        成功返回完整Block，由execute_direct_operation调用AgentMcl.create_block原子写入

parse_ref_merges(agent_mcl: &AgentMcl, declarations: &[RefMergeDeclaration]) -> Result<RefBlock, MclError>
    CREATE REF_BLOCK初始化解析：
        每项调用AgentMcl.ref_merge，验证来源只指向真实Block字段且类型一致
        拒绝重复merge_id；所有引用先写入临时RefBlock，任一项失败时不创建RefBlock
        成功返回完整RefBlock，由execute_direct_operation调用AgentMcl.create_ref_block原子写入

execute_merge(world: &World, agent_id: &ResourceId, sources: &[BlockPath]) -> Result<MclCommandValue, MclError>
    顶层MERGE：读取目标Agent的AgentMcl并调用merge，按来源顺序返回MclCommandValue::Inner；不修改AgentMcl

execute_ref_merge(world: &World, agent_id: &ResourceId, sources: &[BlockPath]) -> Result<MclCommandValue, MclError>
    顶层REF_MERGE：读取目标Agent的AgentMcl并调用ref_merge，再克隆RefMerge.paths返回MclCommandValue::Paths；不读取字段内容且不修改AgentMcl

execute_inject_to(world: &mut World, agent_id: &ResourceId, target: BlockPath, binding: MclBinding) -> Result<MclCommandValue, MclError>
    INJECT TO执行：
        使用world.entity_by_resource_id(agent_id)
        从Entity读取Agent组件并取得AgentMcl
        查找目标Block和字段，读取目标InnerType
        将binding解析为只含一个元素的BlockInner
        调用Agent组件内AgentMcl.insert(&target, parsed_values)
        修改成功后调用publish_realtime_after_mutation(world, agent, &target)，再返回MclCommandValue::Unit；任一步MCL验证失败都不提交部分修改

execute_select_from(world: &World, agent_id: &ResourceId, target: BlockPath) -> Result<MclCommandValue, MclError>
    SELECT FROM执行：
        使用world.entity_by_resource_id(agent_id)查找唯一Entity
        从Entity读取Agent组件，再取得AgentMcl只读引用
        调用AgentMcl.select(&target)
        AgentMcl先按target.block_id查找真实Block；命中时克隆target.inner_id字段
        未命中真实Block时按同一ID查找RefBlock；命中时把target.inner_id作为merge_id并调用RefMerge迭代器
        将克隆结果转换为MclCommandValue并返回
        查询失败返回错误；不修改AgentMcl，不发送MclDomainRequest或MclDomainResponse

execute_inject_cover(world: &mut World, agent_id: &ResourceId, target: BlockPath, binding: MclBinding) -> Result<MclCommandValue, MclError>
    INJECT绑定覆盖执行：
        使用world.entity_by_resource_id(agent_id)
        从Entity读取Agent组件并取得AgentMcl
        查找目标字段并按InnerType解析binding为单元素BlockInner
        调用Agent组件内AgentMcl.cover(&target, parsed_values)
        覆盖成功后调用publish_realtime_after_mutation(world, agent, &target)，再返回MclCommandValue::Unit；失败时原字段保持不变

execute_inject_select_cover(world: &mut World, agent_id: &ResourceId, source: BlockPath, target: BlockPath) -> Result<MclCommandValue, MclError>
    INJECT SELECT覆盖执行：
        使用world.entity_by_resource_id(agent_id)
        从Entity读取Agent组件并取得AgentMcl
        查找源字段和目标字段，验证BlockInner类型完全一致
        克隆源字段完整BlockInner
        调用Agent组件内AgentMcl.cover(&target, cloned_values)
        覆盖成功后调用publish_realtime_after_mutation(world, agent, &target)，再返回MclCommandValue::Unit；任一路径不存在或类型不一致时不提交修改

execute_delete(world: &mut World, agent_id: &ResourceId, target: BlockPath, operation: MclOperation) -> Result<MclCommandValue, MclError>
    DELETE执行：
        使用world.entity_by_resource_id(agent_id)
        从Entity读取Agent组件并取得AgentMcl
        查找目标Block和字段
        DeleteAll转换为MclDeleteSelection::All；空字段按幂等成功处理
        DeleteFirst转换为MclDeleteSelection::First；空字段返回错误
        DeleteWhere要求目标为ToolCall字段并按ToolCall.id比较字符串谓词，再把全部匹配索引转换为MclDeleteSelection::Indices；无匹配项按幂等成功处理
        调用Agent组件内AgentMcl.delete(&target, selection)
        删除成功后调用publish_realtime_after_mutation(world, agent, &target)，再返回MclCommandValue::Unit；AgentMcl.delete原子删除并保持剩余元素顺序
```

私有：
```text
with_agent_mcl_mut<R>(world: &mut World, agent_id: &ResourceId, operation: impl FnOnce(&mut AgentMcl) -> Result<R, AgentError>) -> Result<R, MclError>
    访问AgentMcl：调用WorldResourceIdExt::entity_by_resource_id找到Entity，读取Entity上的Agent组件，取得Agent.mcl_mut()并调用operation
    行为：资源ID不存在或重复时转换为MclError；Entity缺少Agent组件时返回AgentMissing；不暴露内部程序集

with_agent_mcl<R>(world: &World, agent_id: &ResourceId, operation: impl FnOnce(&AgentMcl) -> Result<R, AgentError>) -> Result<R, MclError>
    只读访问AgentMcl：按完整agent_id找到唯一Agent并调用operation；统一把Agent寻址及AgentError转换为MclError

resolve_block(assembly: &BlockAssembly, block_id: &str) -> Result<&Block, MclError>
    解析真实Block：不存在时返回BlockMissing

resolve_inner(block: &Block, inner_id: &str) -> Result<&BlockInner, MclError>
    解析字段：不存在时返回InnerMissing

parse_operation(command: &str, binding: Option<&serde_json::Value>) -> Result<MclOperation, MclError>
    解析命令：私有函数，把且只把公开语法中的一条完整命令转换成对应MclOperation
    行为：
        INJECT值、INJECT COVER和DELETE WHERE必须恰好使用一个binding；缺失返回BindingMissing
        DELETE WHERE要求binding为字符串并构造MclPredicate::ToolCallIdEquals
        其他非Effect命令携带binding时返回InvalidCommand，避免未消费输入
        CREATE中的全部字段初始化器在此解析为BlockFieldDeclaration或RefMergeDeclaration，但不读取World、不修改AgentMcl
        CREATE中的MESSAGE、TOOL_CALL和TOOL分别映射为InnerType::Message、InnerType::ToolCall和InnerType::ResourceId
        Effect只解析名称和语法形状；带RefBlock参数的Effect保存ref_block_id，领域执行阶段再从目标Agent的AgentMcl展开
        tool_call把唯一binding解析为完整ToolCall数组，history_append解析单个MclMessage；其余Effect拒绝未消费的binding

validate_binding(binding: &MclBinding, expected: InnerType) -> Result<BlockInner, MclError>
    验证绑定：转换为只包含一个MclMessage、ToolCall或ResourceId元素的BlockInner

resolve_effect_request(agent_mcl: &AgentMcl, ref_block_id: &str) -> Result<MclEffectRequest, MclError>
    展开Effect请求：私有函数
    行为：
        从AgentMcl.ref_blocks查找ref_block_id，不按名称赋予任何业务语义
        按RefMerge的InnerType分类；要求恰有一个Message RefMerge、至多一个ResourceId RefMerge且不允许ToolCall RefMerge
        分别调用RefMerge.iter取得当前Block内容；Message和ResourceId保持各自路径声明顺序
        使用Message RefMerge的merge_id和paths构造MclRealtimeSource，并返回消息、资源和来源

publish_realtime_after_mutation(world: &mut World, agent: Entity, changed: &BlockPath)
    发布字段变更后的实时快照：私有函数
    行为：
        读取Agent.mcl.realtime_source；未声明或dependencies不包含changed时直接返回
        重新按source.ref_block_id和message_merge_id定位RefMerge并迭代全部依赖，得到完整Vec<MclMessage>
        发送AgentRealtimeContextWriteRequested { agent, messages }，MemoryPlugin用该快照整体覆盖实时上下文表
        不发送增量、不根据Block或字段名称补充内容；同一条命令只在成功提交字段变更后发布一次
```

## 逻辑

```text
领域完成：
    CREATE、MERGE、REF_MERGE、INJECT、SELECT、COVER和DELETE由命令请求System直接执行并直接完成MclCommandReply，不产生领域请求或领域响应
    IMPORT和全部EMIT EFFECT才进入MclDomainRequest链
    Inference、ToolCall、Finish、HistoryAppend和RealtimeSource成功提交各自操作后返回Unit
    IMPORT把原reply保存在PendingMclImports，AgentResourceRegisterResponse到达后提交并完成回执
    Start、CatchInference和RealtimeLoad把原reply保存在PendingMclEffects，等待严格匹配的外部响应
    后续普通Agent轮次结果不属于发起该轮次的MCL命令回执

七项Effect闭环：
    realtime_load -> PendingMclEffects -> AgentRealtimeContextReadRequested
                  -> AgentRealtimeContextReadCompleted -> Inner(Message[]) -> 原reply
    realtime_source(ref_block) -> 保存MclRealtimeSource -> AgentRealtimeContextWriteRequested(完整快照) -> Unit -> 原reply
        后续INJECT/COVER/DELETE命中dependencies -> 重新展开完整快照 -> AgentRealtimeContextWriteRequested
    start -> PendingMclEffects -> LuaVmMessageReceiveRequest
          -> 等待AgentMessage经AgentPlugin写入VM邮箱 -> LuaVmMessageReceived -> Message -> 原reply
    catch_inference(ref_block) -> PendingMclEffects -> CapturedInferenceRequest
                                -> CapturedInferenceResponse -> Text -> 原reply
        整条链没有AgentMessage，因此不会进入AgentPlugin的agent_message_system
    inference(ref_block) -> InferenceRequestEvent -> Unit -> 原reply
        模型响应 -> AgentMessage::Assistant -> AgentPlugin -> VM邮箱 -> 后续start
    tool_call(binding) -> ToolCallEvent[] -> Unit -> 原reply
        每个工具响应 -> AgentMessage::Tool -> AgentPlugin -> VM邮箱 -> 后续start
    finish -> Agent.turn.finish -> Unit -> 原reply；Base Lua循环回到下一次start
    每项同步失败都沿当前MclDomainResponse完成原reply；等待型响应无论Ok或Err都先移除pending再完成原reply，不存在遗失回执的分支

非Effect指令闭环：
    CREATE BLOCK -> parse_block_fields -> AgentMcl.create_block -> Unit -> 原reply
    CREATE REF_BLOCK -> parse_ref_merges -> AgentMcl.create_ref_block -> Unit -> 原reply
    MERGE -> AgentMcl.merge -> Inner -> 原reply
    REF_MERGE -> AgentMcl.ref_merge -> Paths -> 原reply
    INJECT TO -> validate_binding -> AgentMcl.insert -> Unit -> 原reply
    INJECT COVER -> validate_binding -> AgentMcl.cover -> Unit -> 原reply
    INJECT SELECT COVER -> AgentMcl.select -> AgentMcl.cover -> Unit -> 原reply
    SELECT -> AgentMcl.select -> Inner -> 原reply
    DELETE -> AgentMcl.delete -> Unit -> 原reply
    IMPORT -> MclDomainRequest -> PendingMclImports -> AgentResourceRegisterRequest
           -> AgentResourceRegisterResponse -> commit_import -> MclDomainResponse -> 原reply
    任一步同步失败沿当前链立即返回Err；IMPORT提交失败通过MclDomainResponse返回Err；不存在无回执分支

初始化：
    AgentPlugin创建Agent时创建空AgentMcl
    MclPlugin等待命令创建Block、RefBlock和资源别名
    MclPlugin不提前创建任何Block、字段、引用、默认数组或保留名称

Agent目标：
    每个MclCommandRequest必须显式携带完整agent_id
    MclPlugin不保存或推断当前Agent，不鉴权调用方是否允许操作目标Agent
    是否允许跨Agent命令由入口环境决定

调用与返回：
    每条MclCommandRequest最终都必须通过MclCommandReply返回Result
    MclPlugin不提供无回执、仅发送或后台执行形式的命令
    调用方如何等待回执不改变MCL领域语义

持久化：
    RealtimeLoad显式读取实时上下文表并返回有序MclMessage数组
    RealtimeSource保存RefMerge路径作为权威实时来源，声明本身立即发布一次完整快照
    只有声明RealtimeSource后才在其dependencies中的BlockPath成功变化时重新展开并整体覆盖实时表
    MclPlugin不根据Block名称推断实时来源

不变量：
    所有Block、字段、RefBlock和merge名称均来自MCL源码
    BlockInner数组必须同类型并保持声明和操作产生的顺序
    RefMerge只能引用同类型字段，不能作为写入目标
    真实Block和RefBlock共享block_id命名空间，创建时禁止重名
    引用解析读取当前Block内容，不缓存旧数组
    Effect参数必须由公开语法中的RefBlock ID或binding显式产生，宿主不得猜测req、com、ctx或vis
    MCL命令失败时不提交部分修改
    未声明的查询、覆盖、删除和Effect参数必须报错
```
