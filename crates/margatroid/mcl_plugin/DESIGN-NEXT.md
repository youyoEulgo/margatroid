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

本文记录当前已实现的明确类型资源导入与数组表达式契约。Rust使用MESSAGE/TOOL/SANDBOX，旧RESOURCE/TOOL_CALL字段及省略声明类型的IMPORT已拒绝。队列、生命周期、oneshot及历史基础保持；资源加载通过受检主线程原子挂载与绑定发布。DESIGN.md保留重写前7c9055c的归档参考。

正文按lib、system、handler、events、types、error六板块组织。parser源码职责在handler板块描述，不另加第七板块。

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。

# lib

## 类型

公开：
```text
MclPlugin：MCL命令插件，公开结构体
    schedule: String--唯一命令System所在计划
    impl Plugin for MclPlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：安装共享Driver环境factory与私有运输回执表，挂载唯一mcl_command_system，不依赖其他领域Plugin的Handler。

MclEnvironmentFactory：Driver环境工厂，公开共享Resource
    build(&self) -> LuaEnvironment
        构造环境：公开方法
        行为：提供mcl宿主函数，捕获事件发送能力，不保存World借用；wait由LuaRuntime直接提供。
```

## 逻辑

```text
Driver输入：
    mcl(target_resource_id, text, arguments)
    arguments始终为外层参数列表，每项为对应一个问号的完整数组
    零问号省略第三参数，不能显式传nil；一个问号也不省略外层
    Lua空table仅在外层列表或已明确的手动数组位置转换为空数组，消息内部对象不递归猜测

资源能力：
    TOOL：tool、skill、hook的可调用资源值，不是模型请求ToolCall结构
    MESSAGE：AgentMessage数组，prompt按自身role转换为消息，不按目标字段猜角色
    SANDBOX：沙箱资源值数组
    资源地址仍统一为ResourceID，但RESOURCE不再是Block或GET的数组类型
    IMPORT只加载、验证并命名，不自动注入上下文、不自动开放模型工具、不自动激活沙箱
    Agent.import_bindings保存单一alias绑定事实，资源正文仍由ResourceSet持有；不可变内容通过Arc共享
```

# system

## 函数

公开：
```text
mcl_command_system(world: &mut World)
    命令System：公开System
    处理事件：MclCommand、InternalMclHostSubmission、InternalMclCancellationSignal、MclAsyncOutput
    产出事件：领域加载/推理/工具请求、MclCommandResult
    行为：
        第一阶段绑定可信来源并逐条受理入目标Agent FIFO
        第二阶段收口匹配的异步队首并派发回执，不保存任何message历史
        第三阶段处理停止或来源取消，再推进各Agent队首
        同Agent异步等待不执行后续项，不同Agent独立推进；同步预算每Agent每帧64条
```

## 逻辑

```text
回执与关闭：
    Driver回执先通过私有HostReply对应oneshot同步发送，再发布公开观察结果
    commands/pending为空且所有回执派发后才mark_drained
    Stopping或执行作用域关闭拒绝新命令，未执行项Cancelled，在途取消但等待真实收口
    来源Driver关闭时只取消其完整实例关联的目标命令，取消signal唤醒事件驱动调度而不提供授权
    ready只允许当前Driver对本人调用；slot已清但实例匹配时返回已交付信息
```

# handler

## 函数

私有：
```text
parse_operation(instruction: &MclInstruction) -> Result<MclOperation, MclError>
    解析指令：私有函数
    行为：
        验证完整文本、关键字、括号与手动参数数量，不读取World
        CREATE BLOCK字段仅MESSAGE、TOOL、SANDBOX，拒绝RESOURCE和TOOL_CALL
        IMPORT严格为IMPORT <MESSAGE|TOOL|SANDBOX> <ResourceID> AS <alias>
        GET及INJECT左侧解析为同一种有序数组表达式，effect括号也使用此表达式
        不受理REF、MERGE、STATE、EXPOSE和旧visibility来源操作

parse_array_expression(text: &str) -> Result<ArrayExpression, MclError>
    解析数组表达式：私有函数
    行为：
        仅按方括号之外的逗号切分，拒绝空项、尾逗号及不配对括号
        每项为已导入alias或普通Block选择器；完整ResourceID仅在IMPORT中建立绑定，不新增裸地址读取语法
        alias采用首字符字母或下划线、后续字母数字下划线或连字符，不允许点以避免与block.field混淆
        选择器保留字段路径及原范围，不在解析阶段求值或冻结快照

begin_import(world: &mut World, request: &MclCommand, kind: InnerType, resource_id: ResourceID, alias: String) -> Result<(), MclError>
    发起明确类型导入：私有函数
    行为：
        TOOL仅允许tool/skill/hook，MESSAGE仅允许prompt，SANDBOX仅允许sandbox，不匹配立即拒绝
        resource_id须精确属于目标Agent.info.image_dependencies，无builtin隐含例外
        alias在一个Agent内跨所有类别唯一，已有绑定只能由同一ResourceID及同一声明类型重载；其他资源或类型占用则AliasConflict
        准备对应资源加载请求并移交Other活动，Sender槽和取消能力随请求，MCL等待唯一Receiver
        请求携带import_kind: Some(kind)，加载方主线程收口核对完整Entity、取消能力及导入许可，构造ImportedValue快照并原子挂载与发布绑定；MCL仅等待oneshot，不在后续帧二次绑定
        MESSAGE从PromptResource.role/content构造消息数组，system必须是System消息，不降级为User
        TOOL构造带原ResourceID/alias及不可变调用定义的CallableResource；SANDBOX构造SandboxValue
        资源替换及绑定发布通过同一受检主线程收口，加载或类型/绑定校验失败不覆盖旧条目和旧绑定
        每次IMPORT重读资源，即使同ID已存在；ID在同Agent采用一个本地alias，同ID改alias时移除旧名，冲突先检查
        IMPORT完成仅返回类型、资源ID和alias，不自动INJECT或执行effect
        取消后禁止挂载或发布新绑定，真实加载收口后才释放队首；观察Result不参与MCL等待

evaluate_array_expression(world: &World, agent: Entity, expression: &ArrayExpression) -> Result<BlockInner, MclError>
    统一数组求值：私有函数--GET、INJECT左项与effect括号唯一读取入口
    行为：
        核对完整Agent实例，按书写顺序读取每个来源
        Alias从Agent.import_bindings取ImportedValue，转换为Message/Tool/Sandbox typed数组
        Selector读取Agent.block对应字段，按范围复制值；只读取普通Block，不展开引用视图
        所有来源必须同类型，空区间也参与类型检查；按顺序拼接，保留重复元素，不隐式去重
        单索引仍返回数组，消息不包装为单Message，空结果不变为Unit
        返回此刻快照，不建立自动更新关系；重载后重新GET alias取新版本，旧Block值保持旧快照
        任何alias/字段缺失、混类型或范围错误均丢弃临时结果，无写入副作用

execute_get(world: &World, agent: Entity, expression: &ArrayExpression) -> Result<MclValue, MclError>
    执行读取：私有函数
    行为：直接调用evaluate_array_expression并返回Array，不按资源地址或调用方改变结果类型。

prepare_inject(world: &World, agent: Entity, source: &ArrayParameter, target: &Selector, arguments: &[Value]) -> Result<PreparedInject, MclError>
    准备注入快照：私有函数
    行为：
        表达式来源直接调用与GET相同的evaluate_array_expression，不发送新GET事件
        手动MESSAGE问号严格解码完整消息数组；手动TOOL/SANDBOX仅接受已导入alias字符串数组，通过同一受检读取入口获得资源快照，拒绝裸ID及脚本/schema/policy对象；Empty按目标类型构造空数组
        求值结果必须与目标字段同类型，不将TOOL/SANDBOX转Message、不将Message转资源
        全部读取、类型与范围检查成功后返回唯一目标路径及替换快照，执行阶段一次replace
        来源与目标相同也先读完所有来源，后续来源不读到本次修改

evaluate_effect_parameters(world: &World, agent: Entity, operation: &MclOperation, arguments: &[Value]) -> Result<Vec<BlockInner>, MclError>
    求值效果参数：私有函数
    行为：
        每个括号独立调用evaluate_array_expression；MESSAGE问号严格解码消息数组，TOOL/SANDBOX问号以alias字符串数组读取已导入快照，空数组保留要求类型
        inference依次要求MESSAGE和TOOL，tool_call依次要求MESSAGE和TOOL
        history_append要求MESSAGE，sandbox_use要求SANDBOX，wake/ready/start_turn/finish为零参数
        先检查全部数量与类型再提交，无部分执行，参数之间不拼接

begin_tool_call(world: &mut World, request: &MclCommand, messages: Vec<AgentMessage>, visible: Vec<CallableResource>) -> Result<(), MclError>
    执行assistant工具调用：私有函数
    行为：
        第一参数必须恰好一条Assistant，从其tool_calls读取调用请求，非assistant或多条消息拒绝
        工具调用ID和名称须非空，批次ID唯一；TOOL数组alias冲突整批准备失败，不按遍历顺序任选
        tool_calls为空直接返回typed空MESSAGE数组，不提交任务
        按每个tool_name在visible快照中匹配alias，无匹配填对应失败消息
        每个可调用项发送一个ToolExecuteRequest，携带选定CallableResource快照及原调用ID/参数/turn，不按ID重新取最新定义
        Hook允许导入为TOOL，但具体触发/执行能力未闭合；当前不把Hook当普通Lua runner或假装执行成功，按不支持项完成明确失败
        逐项等待oneshot，失败或通道关闭填该项失败结果，按原tool_calls顺序收集MESSAGE数组
        结果只作为MESSAGE数组返回，包括拒绝和失败项；输入assistant与输出tool_call消息都不隐式写历史，保存必须显式history_append
        默认只有MCL指令审计记录，不自动产生N条工具message记录；Driver决定哪些消息显式history_append，不生成调用数组Block字段
```

## 逻辑

```text
语法示例：
    IMPORT TOOL tool:local/bash:latest AS bash
    IMPORT TOOL skill:local/review:latest AS review
    IMPORT TOOL hook:local/review-flow:latest AS review-flow
    IMPORT MESSAGE prompt:local/coder-soul:latest AS soul
    IMPORT SANDBOX sandbox:local/workspace-write:latest AS workspace_write
    CREATE BLOCK msg (instructions MESSAGE, conversation MESSAGE)
    CREATE BLOCK setting (visible TOOL, activated SANDBOX)
    GET soul, compact
    GET bash, review
    GET workspace_write
    GET msg.instructions, soul
    GET bash, soul--类型错误
    INJECT soul, compact TO msg.instructions
    INJECT bash, review TO setting.visible
    INJECT workspace_write TO setting.activated
    EMIT EFFECT inference (msg.instructions, msg.conversation) (setting.visible)
    EMIT EFFECT tool_call ? (setting.visible)
    EMIT EFFECT history_append ?
    EMIT EFFECT sandbox_use (setting.activated)

Driver工具回合：
    assistant = inference结果.message
    command("EMIT EFFECT tool_call ? (setting.visible)", {{assistant}})
    外层是arguments，内层单元素MESSAGE数组；不接受单对象特殊封装
    返回MESSAGE工具结果数组；Driver分别决定INJECT上下文和显式history_append，二者互不隐含
    删除work.calls及TOOL_CALL字段，不再单独INJECT tool_calls数组

范围与拼接：
    All=[0,len]，Empty=[0,0]，最终左闭右开；负端点加len+1
    非负Read[x]=[x,x+1]，负Read[x]=[len+x,len+1+x]
    非负Write[x]=[x,x]，负Write[x]=[len+1+x,len+1+x]
    空字段Read[0]返回typed空数组；双开放端点/显式混号/逆序/越界/溢出拒绝
    INJECT[] TO b.f清空字段，INJECT[] TO b.f[]只替换空区间
    GET soul,compact为MESSAGE，GET bash,review为TOOL，GET workspace_write为SANDBOX
    混合类型即使包含空数组也报错，不在INJECT目标上重新解释来源

沙箱：
    sandbox_use只接受SANDBOX数组，零项取消、一项激活、多项失败不修改
    策略快照来自选定SandboxValue，必须核对所属Agent导入许可，不从全局首个匹配者取值
    真实策略改变受Sandbox活动及受理门限制，Inference/Other不阻止
    选中旧快照即使用旧策略，不因同ID集合重载暗换；相同ID但不同policy属于配置改变

历史：
    history_append只有完整参数求值与递归允许类型检查成功后才豁免命令记录，准备快照只追加一次
    非法history_append先审计原text和arguments再返回校验错误，不追加消息；合法写库失败不补写命令或重试
    除合法history_append外，执行前记录原text和展开arguments，失败不执行
    message历史唯一写入入口是显式history_append，包括用户、assistant、工具成功/失败、error及inject消息；推理、工具、GET、INJECT和IMPORT不写message记录
    history_append按批次事务保存，写入失败返回本条history_append错误；推理和工具原结果不会因此重做
    保留MCL指令审计自动记录，参数展开中的消息数据只是审计payload，不额外投影为message记录
    history_error保留为取消项审计失败等附带错误，不再表示推理或工具自动结果message保存失败
    history_append允许inject/user/assistant/tool_call/error，System值可用于上下文但不通过此effect新增记录；递归检查Inject中的System，拒绝时不追加任何记录
    inject内部结构仍按完整AgentMessage契约，历史输入允许范围规则独立于消息值类型
```

# events

## 类型

公开，全部具名载荷统一在共享xxx_events.rs：
```text
MclCommand：命令请求，公开事件
    id: String
    source: ResourceID
    target: ResourceID
    command: MclInstruction
    impl Event for MclCommand

MclCommandResult：命令结果，公开事件
    id: String
    source: ResourceID--原值不交换
    target: ResourceID--原值不交换
    result: Result<MclValue, MclError>
    history_error: Option<HistoryError>
    impl Event for MclCommandResult

InferenceRequest：统一推理请求，公开事件
    tools: Vec<CallableResource>--替代resources:Vec<ResourceID>，定义于inference_events
    说明：其他关联、消息、取消与oneshot字段沿用领域契约，不由MCL复制完整结构。

ToolExecuteRequest：单调用执行请求，公开事件
    resource: CallableResource--选定共享快照，自含ResourceID，不另建重复resource_id字段
    call: ToolCall--仅来自输入assistant.tool_calls
    说明：其余agent、turn、cancel、reply字段沿用共享tool_events。
```

# types

## 类型

公开共享契约：
```text
InnerType：字段类型，公开枚举
    Message
    Tool
    Sandbox

BlockInner：字段数组，公开枚举
    Message(Vec<AgentMessage>)
    Tool(Vec<CallableResource>)
    Sandbox(Vec<SandboxValue>)

CallableResource：可调用资源值，公开结构体
    resource_id: ResourceID--具体资源身份
    alias: String--本地模型调用名
    definition: Arc<CallableDefinitionSnapshot>--描述、schema及Tool/Skill/Hook具体执行材料快照

SandboxValue：沙箱资源值，公开结构体
    resource_id: ResourceID
    policy: Arc<str>--不可变策略快照

ImportedValue：明确类型绑定，公开枚举
    Message { resource_id: ResourceID, messages: Arc<[AgentMessage]> }
    Tool(CallableResource)
    Sandbox(SandboxValue)

ImportBindings：Agent本地alias绑定，公开共享类型
    bindings: BTreeMap<String, ImportedValue>--私有唯一命名权威，公开get及受检WorldAgentExt访问，不复制另一份全局alias索引

ArrayExpression：有序读取表达式，私有类型
    sources: Vec<ArraySource>

ArraySource：读取来源，私有枚举
    Alias(String)
    Selector(Selector)

ArrayParameter：完整数组参数，私有枚举
    Expression(ArrayExpression)
    Manual { index: usize }
    Empty--仅INJECT[]，不新增内联JSON文本数组语法

ToolCall：assistant字段纯值，公开结构体
    id: String
    tool_name: String
    arguments: String
    说明：保留消息和Provider请求结构，但删除作为Block数组、GET结果或effect独立调用数组的用途。
```

# error

## 逻辑

```text
新增目标错误必须区分导入声明类型不匹配、alias绑定冲突、alias未导入、数组混型及资源能力不支持。
普通错误完成本请求，不自动终止整个Driver，不以领域异常代替指令回执。
已有停止、取消、幂等历史、history_error及完整Entity关联保持不变。
本轮仍未实现Hook调用语义；允许声明TOOL不等于所有执行插件已支持其触发路线。
旧RESOURCE/TOOL_CALL字段已迁移；Driver工具回合直接传单条Assistant消息数组，不保留work.calls。宿主与网络输出采用安全投影，不序列化完整调用执行材料或沙箱策略。
```
