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

本轮已实现：所有运行时Block统一存真实数据库，CREATE/GET/INJECT及包含选择器的参数求值异步事务化，旧effect `finish` 已删除并改为 `finish_turn`。明确类型资源导入与数组表达式契约保持，Rust使用MESSAGE/TOOL/SANDBOX。队列、完整实例生命周期、oneshot及历史基础保持；资源加载通过受检主线程原子挂载与绑定发布。

正文按lib、system、handler、events、types、error六板块组织。parser源码职责在handler板块描述，不另加第七板块。

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。message记录的content保存实际正文（Error保存message文本，Inject为空，Assistant无正文为空）；payload只保存type及其余元数据，不重复顶层content/Error.message。Inject的messages保留完整嵌套消息结构。旧行原样读取，由消费者兼容payload中的旧正文，不破坏性迁移。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。

# 实际动作受理与中止

MCL已实现Creating / Idle / Working / Stopping轮次effect。start_turn要求Idle，生成turn并置Working，不覆盖；finish_turn核对准备阶段捕获的turn或当前实例最近中止关联，并要求该轮所有execution注册真实释放后才进入Idle。Idle完成幂等，Stopping不能被finish覆盖。inference/tool_call在异步准备后的实际动作核对当前turn；空turn或已失效捕获turn返回正常取消结果，不发领域请求。原StopAgent及Creating/Stopping真实清理契约不变。AgentPlugin控制System使用PRE_UPDATE，领域与MCL使用UPDATE；自定义schedule须保留控制优先。下列Driver/领域/协议跨插件规则需结合各自实现验证。

```text
控制与受理：
    abort为独立控制事件，不是MCL命令、不走FIFO；控制System先于MCL推进及推理/工具受理
    abort在Working立即清agent.turn并请求所属轮次取消，Working保持至Driver finish_turn
    Working+turn=None重复abort幂等pending；前端仅Idle可发送、Working可abort，服务端同样校验用户输入仅Idle可受理
    inference/tool_call在异步审计和参数准备完成后的实际effect执行处检查当前turn
    不在准备开始时冻结turn门；PreparedEffect继续仍须检查，不依赖get_turn effect或Driver检查turn
    无turn时不提交领域执行，返回Ok及正常Inference/工具MESSAGE数组形状的取消结果
    turn存在时携带该轮次关联提交；领域受理与abort控制顺序互斥，不能在清turn后迟到启动执行
    outstanding未受理请求由取消路径claim一次性reply恰好一次，迟到受理不得再执行或回复
    已受理请求取消token，真实清理完成后沿正常结果通道返回；清理失败保持Err及未清理活动，不回Idle
    五种状态以abort前Working且turn存在为前提：inference未受理、inference已受理、tool_call未受理、tool_call已受理、Driver间隙
    前两类按Assistant正常结果收口，中间工具两类按原调用ID逐项收口，Driver间隙由下一effect实际动作的turn门补上
    Working+turn=None重复abort幂等pending另行保持，不是第五类的替代
    finish_turn只完成当前Driver关联结果链且必须本轮真实清理完成；Idle重复完成可幂等，迟到旧完成不得覆盖新Working轮次

取消消息值与结果形状：
    Assistant消息自身携带stop_reason，新增Cancelled枚举并序列化为cancelled
    取消Assistant content为英文'Turn aborted.'，tool_calls为空，usage=None
    inference正常外层成功值及message结构保持，取消不是MclError::Cancelled替代结果
    每个取消工具结果保留原调用ID、原顺序及既有工具身份关联，failed=true，content为'Tool call aborted.'
    无turn的tool_call仍按原Assistant.tool_calls逐项返回取消结果，空批次仍是typed空MESSAGE数组
    stop_reason随消息经过Block、历史和宿主投影，不只停留在推理外层返回值
    真实推理、工具、存储或清理失败继续原错误契约；不为实际失败编造cancelled Assistant，不直接finish_turn
```

普通MCL命令仍按目标FIFO顺序推进；轮次abort的控制优先级不等于停止整个队列，不应把未执行inference/tool_call一律作为FIFO取消错误丢掉。它们实际执行时无turn会生成上述正常取消结果供Driver收尾。停止/来源关闭的既有队列取消与审计收口独立保留。

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
    TOOL：tool的可调用资源值，不是模型请求ToolCall结构
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
    处理事件：MclCommand、InternalMclHostSubmission、InternalMclCancellationSignal、MclAsyncOutput、MclCancelledOutput
    产出事件：领域加载/推理/工具请求、MclCommandResult
    行为：
        第一阶段绑定可信来源并逐条受理入目标Agent FIFO
        第二阶段匹配完整pending identity，Completed派发回执，PreparedEffect/PreparedImport直接继续主线程动作，不重走队首parse或审计；取消审计批次完成后派发其回执
        第三阶段处理停止或来源取消，再推进各Agent队首
        同Agent异步等待不执行后续项，不同Agent独立推进；同步预算每Agent每帧64条
```

## 逻辑

```text
回执与关闭：
    Driver回执先通过私有HostReply对应oneshot同步发送，再发布公开观察结果
    commands/pending为空、cancel_audits为零且所有回执派发后才mark_drained
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
        CREATE BLOCK与CREATE TEMP BLOCK字段仅MESSAGE、TOOL、SANDBOX，拒绝RESOURCE和TOOL_CALL；声明分别选择持久和本次启动隔离的表生命周期
        IMPORT严格为IMPORT <MESSAGE|TOOL|SANDBOX> <ResourceID> AS <alias>
        GET及INJECT左侧解析为同一种有序数组表达式，effect括号也使用此表达式
        不受理REF、MERGE、STATE、EXPOSE和旧visibility来源操作

create_database_block(context: BlockExecutionContext, name: String, fields: Vec<FieldDeclaration>, temporary: bool) -> Result<MclValue, MclError>
    创建或打开数据库Block：私有异步函数
    行为：
        普通Block以稳定AgentResourceID和block名称寻址，TEMP额外使用本次Driver启动实例命名空间
        不存在则在事务中建立定义与typed空字段；已存在且完整字段声明相同则打开，不清空
        同名生命周期、字段数量/名称/类型不匹配返回SchemaMismatch，不静默迁移、合并或覆盖旧数据
        返回BlockOpened {created, temporary}，Driver只在created为true时填默认配置
        所有Block都是真实数据库表/行，不提供另一套内存后端或BIND/SAVE/LOAD同步接口

parse_array_expression(text: &str) -> Result<ArrayExpression, MclError>
    解析数组表达式：私有函数
    行为：
        仅按方括号之外的逗号切分，拒绝空项、尾逗号及不配对括号
        每项为已导入alias或数据库Block选择器；完整ResourceID仅在IMPORT中建立绑定，不新增裸地址读取语法
        alias采用首字符字母或下划线、后续字母数字下划线或连字符，不允许点以避免与block.field混淆
        选择器保留字段路径及原范围，不在解析阶段求值或冻结快照

begin_import(world: &mut World, request: &MclCommand, kind: InnerType, resource_id: ResourceID, alias: String) -> Result<(), MclError>
    发起明确类型导入：私有函数
    行为：
        TOOL仅允许tool，MESSAGE仅允许prompt，SANDBOX仅允许sandbox，不匹配立即拒绝
        resource_id须精确属于目标Agent.info.driver_dependencies，无builtin隐含例外
        alias在一个Agent内跨所有类别唯一，已有绑定只能由同一ResourceID及同一声明类型重载；其他资源或类型占用则AliasConflict
        准备对应资源加载请求并移交Other活动，Sender槽和取消能力随请求，MCL等待唯一Receiver
        请求携带import_kind: Some(kind)，加载方主线程收口核对完整Entity、取消能力及导入许可，构造ImportedValue快照并原子挂载与发布绑定；MCL仅等待oneshot，不在后续帧二次绑定
        MESSAGE从PromptResource.role/content构造消息数组，system必须是System消息，不降级为User
        TOOL构造带原ResourceID/alias及不可变调用定义的CallableResource；SANDBOX构造SandboxValue
        资源替换及绑定发布通过同一受检主线程收口，加载或类型/绑定校验失败不覆盖旧条目和旧绑定
        每次IMPORT重读资源，即使同ID已存在；ID在同Agent采用一个本地alias，同ID改alias时移除旧名，冲突先检查
        IMPORT完成仅返回类型、资源ID和alias，不自动INJECT或执行effect
        取消后禁止挂载或发布新绑定，真实加载收口后才释放队首；观察Result不参与MCL等待

evaluate_array_expression(context: ArrayEvaluationContext, expression: ArrayExpression) -> Result<BlockInner, MclError>
    统一数组求值：私有异步函数--GET、INJECT左项与effect括号唯一读取入口
    行为：
        主线程核对完整Agent并取得AgentDatabaseHandle/导入快照，执行任务按书写顺序读取来源，不携带World借用
        Alias从Agent.import_bindings取ImportedValue，转换为Message/Tool/Sandbox typed数组
        Selector在同一数据库读事务中读取对应Block字段与范围，所有Block来自真实表；不保存独立内存内容或引用视图
        所有来源必须同类型，空区间也参与类型检查；按顺序拼接，保留重复元素，不隐式去重
        单索引仍返回数组，消息不包装为单Message，空结果不变为Unit
        MESSAGE从数据库还原完整消息值；TOOL/SANDBOX字段保存完整ResourceID，读取时按本次Agent导入快照解析当前定义
        普通及TEMP Block都不保存资源定义副本，重载后的下一次GET可取得新导入值；已开始执行的外部请求保持自身提交快照
        ID未导入、kind不匹配或失去许可明确报错，不静默丢失配置，不以旧路径/旧policy恢复
        任何alias/字段缺失、混类型或范围错误均丢弃临时结果，无写入副作用

execute_get(context: ArrayEvaluationContext, expression: ArrayExpression) -> Result<MclValue, MclError>
    执行读取：私有异步函数
    行为：直接调用evaluate_array_expression并返回Array，不按资源地址或调用方改变结果类型。

execute_inject(context: ArrayEvaluationContext, source: ArrayParameter, target: Selector, arguments: Vec<Value>) -> Result<MclValue, MclError>
    事务注入：私有异步函数
    行为：
        数据库工作线程在一个写事务内调用同一来源求值逻辑，不发送新GET事件、不先查后另开写事务
        手动MESSAGE问号严格解码完整消息数组；手动TOOL/SANDBOX仅接受已导入alias字符串数组，通过同一受检读取入口获得资源快照，拒绝裸ID及脚本/schema/policy对象；Empty按目标类型构造空数组
        求值结果必须与目标字段同类型，不将TOOL/SANDBOX转Message、不将Message转资源
        在同一事务中完成全部来源读取、目标长度、类型及范围检查，然后一次replace并提交；错误回滚，不改任何字段
        事务成功提交后才返回Unit并释放队首，不能把已提交成功改成Cancelled；提交事实未知须返回独立错误，不宣称回滚
        来源与目标相同也先读完所有来源，后续来源不读到本次修改

evaluate_effect_parameters(context: ArrayEvaluationContext, operation: MclOperation, arguments: Vec<Value>) -> Result<Vec<BlockInner>, MclError>
    求值效果参数：私有异步函数
    行为：
        全部括号选择器在同一读事务求值，保持独立参数边界；MESSAGE问号严格解码消息数组，TOOL/SANDBOX问号以alias字符串数组读取已导入快照，空数组保留要求类型
        inference依次要求MESSAGE和TOOL，tool_call依次要求MESSAGE和TOOL
        history_append要求MESSAGE，sandbox_use要求SANDBOX，wake/ready/start_turn/finish_turn为零参数
        先检查全部数量与类型再提交，无部分执行，参数之间不拼接

begin_tool_call(world: &mut World, request: &MclCommand, messages: Vec<AgentMessage>, visible: Vec<CallableResource>) -> Result<(), MclError>
    执行assistant工具调用：私有函数
    行为：
        第一参数必须恰好一条Assistant，从其tool_calls读取调用请求，非assistant或多条消息拒绝
        工具调用ID和名称须非空，批次ID唯一；TOOL数组alias冲突整批准备失败，不按遍历顺序任选
        已实现：异步准备后实际动作处要求turn；无turn按全部原调用返回正常形状取消结果，不提交任务；存在turn才允许提交并在领域受理处防迟到启动
        tool_calls为空直接返回typed空MESSAGE数组，不提交任务
        按每个tool_name在visible快照中匹配alias，无匹配填对应失败消息
        每个可调用项发送一个ToolExecuteRequest，携带选定CallableResource快照及原调用ID/参数/turn，不按ID重新取最新定义
        逐项等待oneshot，失败或通道关闭填该项失败结果，按原tool_calls顺序收集MESSAGE数组
        结果只作为MESSAGE数组返回，包括拒绝和失败项；输入assistant与输出tool_call消息都不隐式写历史，保存必须显式history_append
        默认只有MCL指令审计记录，不自动产生N条工具message记录；Driver决定哪些消息显式history_append，不生成调用数组Block字段
```

## 逻辑

```text
语法示例：
    IMPORT TOOL tool:local/bash:latest AS bash
    IMPORT TOOL tool:local/review:latest AS review
    IMPORT MESSAGE prompt:local/coder-soul:latest AS soul
    IMPORT SANDBOX sandbox:local/workspace-write:latest AS workspace_write
    CREATE TEMP BLOCK msg (instructions MESSAGE, compact_prompt MESSAGE)
    CREATE BLOCK session (conversation MESSAGE, visible TOOL, activated SANDBOX)
    CREATE TEMP BLOCK entry (pending MESSAGE)
    EMIT EFFECT finish_turn
    GET soul, compact
    GET bash, review
    GET workspace_write
    GET msg.instructions, soul
    GET bash, soul--类型错误
    INJECT soul, compact TO msg.instructions
    INJECT bash, review TO session.visible
    INJECT workspace_write TO session.activated
    EMIT EFFECT inference (msg.instructions, session.conversation) (session.visible)
    EMIT EFFECT tool_call ? (session.visible)
    EMIT EFFECT history_append ?
    EMIT EFFECT sandbox_use (session.activated)

Driver工具回合：
    assistant = inference结果.message
    command("EMIT EFFECT tool_call ? (session.visible)", {{assistant}})
    外层是arguments，内层单元素MESSAGE数组；不接受单对象特殊封装
    返回MESSAGE工具结果数组；Driver分别决定INJECT上下文和显式history_append，二者互不隐含
    删除work.calls及TOOL_CALL字段，不再单独INJECT tool_calls数组

范围与拼接：
    All=[0,len]，Empty=[0,0]，最终左闭右开；负端点加len+1
    非负Read[x]=[x,x+1]，负Read[x]=[len+x,len+1+x]
    非负Write[x]=[x,x]，负Write[x]=[len+1+x,len+1+x]
    空字段Read[0]返回typed空数组；双开放端点/显式混号/逆序/越界/溢出拒绝
    INJECT [] TO b.f清空字段，INJECT [] TO b.f[]只替换空区间
    GET soul,compact为MESSAGE，GET bash,review为TOOL，GET workspace_write为SANDBOX
    混合类型即使包含空数组也报错，不在INJECT目标上重新解释来源

沙箱：
    sandbox_use只接受SANDBOX数组，零项取消、一项激活、多项失败不修改
    策略快照来自选定SandboxValue，必须核对所属Agent导入许可，不从全局首个匹配者取值
    真实策略改变受Sandbox活动及受理门限制，Inference/Other不阻止
    已激活值及执行中的请求保持选定策略快照；数据库SANDBOX字段只保存ID，下一次GET解析当前导入值，再显式sandbox_use才切换；同ID不同policy是配置改变

数据库与恢复：
    所有Block操作通过AgentBlockStorageHandle异步执行，SQLite在阻塞工作线程，不在ECS执行I/O
    命令开始登记Other活动，取消请求仍等待实际工作线程退出和事务结局，不丢Future假称回滚
    同一Agent只使用其一份数据库，GET多来源读事务、INJECT全部来源与目标同写事务
    alias及手动参数在任务提交时准备不可变快照；数据库TOOL/SANDBOX ID按该次导入快照解析，不跨World借用
    非选择器的alias读取本身无SQL，但CREATE/GET/INJECT统一异步入口；effect动作本身在主线程同步通知，但每条命令的审计准备异步；带字段参数先一次异步读取所有独立参数再继续执行
    History与Block共文件分表，INJECT不写message历史，history_append不修改Block，两条指令不承诺共同事务
    TEMP entry按启动实例隔离，不恢复未处理请求；持久session恢复context及配置，不恢复turn/pending/VM活动
    启动先IMPORT，再打开持久session并检查created，仅初建初始化visible/sandbox；TEMP prompt每次初始化
    读取session.activated后显式sandbox_use，再ready；打开Block不自动激活沙箱或执行旧tool_calls

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

## Driver分支契约（Driver实现由应用层提供）

```text
处理entry：
    读取entry.pending第一条原始消息，完成本次取出后移除同一第一条，不取末条或整批清空
    显式history_append保留原始消息值，不用改写后的上下文值冒充原始历史
    每个分支自行决定conversation更新，不在分支前统一追加所有消息

Inject：
    显式保留原Inject容器历史，inject_context递归展开嵌套Inject并追加非容器消息到conversation
    不递归执行User/Assistant等控制分支，不start_turn、不inference、不finish_turn；嵌套cancelled Assistant同样跳过上下文，原始嵌套历史不改写
User：
    显式追加原始历史，追加conversation，start_turn，然后inference
Assistant：
    显式追加原始历史
    stop_reason=cancelled时仅历史，不追加conversation
    正常Assistant追加conversation
    tool_calls为空时调用finish_turn，这是Driver唯一finish_turn路径
    否则调用tool_call批次，不检查turn；无turn由effect返回逐调用取消结果
工具结果批次：
    显式追加全部原始工具结果历史，并将全部结果追加conversation（包括failed取消结果）
    跟踪本批完成，全部结果齐后发inference，不检查turn或因取消跳过
    即使turn=None也发inference，由实际effect返回cancelled Assistant，走上述无调用分支finish_turn
Error：
    按原错误协议显式保留历史/报告，不直接finish_turn，不为实际失败造cancelled Assistant
```

同一原始输入只按既有显式历史策略记录，不因递归或批次处理重复追加同一条记录。上述取消结果是控制取消的正常值，不是把真实执行失败隐藏成取消；清理失败仍为错误并保留Working，不以恢复Idle掩盖未完成清理。

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

MclValue：命令成功值，公开枚举--保留Array、ResourceImport、Inference、Text与Unit
    BlockOpened { block_id: String, created: bool, temporary: bool }--CREATE创建或打开结果，新建true、同schema打开false；两种Block均使用此值

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
    推理分层（已实现）：MCL继续只提交领域消息与选定工具快照，不选择API格式或厂商，也不直接构造HTTP。InferencePlugin验证后构造共享纯值DomainRequest，再按模型路由provider与api_type转换；InferenceRequest仍为执行事件，不被DomainRequest替代。Driver无需因Chat Completions、Messages或无状态Responses切换而更换。Assistant可选replay随MESSAGE、显式history_append及结果保留；旧值缺字段为None，MCL不解释或伪造签名/加密API元数据。格式转换会拒绝跨格式replay和不可表达的参数，不通过丢失reasoning实现兼容；User和工具结果仍纯文本，图像未实现。

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
    definition: Arc<CallableDefinitionSnapshot>--描述、schema及Tool具体执行材料快照

SandboxValue：沙箱资源值，公开结构体
    resource_id: ResourceID
    policy: Arc<str>--不可变策略快照

ImportedValue：明确类型绑定，公开枚举
    Message { resource_id: ResourceID, messages: Arc<[AgentMessage]> }
    Tool(CallableResource)
    Sandbox(SandboxValue)

ImportBindings：Agent本地alias绑定，公开共享类型
    bindings: BTreeMap<String, ImportedValue>--私有唯一命名权威，公开get及受检WorldAgentExt访问，不复制另一份全局alias索引

StoredBlockInner：数据库字段值，公开共享枚举
    Message(Vec<AgentMessage>)--完整消息结构，inject容器不拆
    Tool(Vec<ResourceID>)--身份列表，不序列化schema/脚本路径/句柄
    Sandbox(Vec<ResourceID>)--身份列表，不序列化policy定义

ArrayEvaluationContext：异步读取准备，私有结构体
    database: AgentBlockStorageHandle--受检取得的数据库视图
    agent_id: ResourceID--持久命名空间
    instance: DriverLuaVmInstance--TEMP命名空间
    imports: ImportBindings--本次解析资源ID所需的不可变导入快照
    cancel: CancellationToken

ArrayExpression：有序读取表达式，私有类型
    sources: Vec<ArraySource>

ArraySource：读取来源，私有枚举
    Alias(String)
    Selector(Selector)

ArrayParameter：完整数组参数，私有枚举
    Expression(ArrayExpression)
    Manual { index: usize }
    Empty--仅INJECT []，不新增内联JSON文本数组语法

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
StorageCommitUnknown明确表示提交事实不确定，不能投影成Cancelled或确认回滚；真实worker已退出且无未清理事务时可完成Other活动。
StorageCleanupFailed表示回滚或事务清理仍未完成；保留Other活动并mark_cleanup_failed，关闭作用域及队列受理，执行作用域wait_all与drain wait返回清理失败，禁止虚假确认stop成功。取消审计本身的CleanupFailed也通过drain.mark_cleanup_failed报告，而不是永久等待无回执。
已有停止、取消、幂等历史、history_error及完整Entity关联保持不变。
旧RESOURCE/TOOL_CALL字段已迁移；Driver工具回合直接传单条Assistant消息数组，不保留work.calls。宿主与网络输出采用安全投影，不序列化完整调用执行材料或沙箱策略。
```

## 目标验收

```text
数据库Block与finish_turn已实现；parser纯函数测试仍可使用AgentBlock作为不执行I/O的输入fixture，Agent运行时不保存该内存容器。
验证重启恢复MESSAGE/TOOL/SANDBOX、TEMP不恢复、schema冲突保留旧数据、跨选择器事务快照、取消与提交事实、历史无隐式联动。
已实现：start_turn仅Idle生成新代号并置Working；abort清turn仍Working；finish_turn经轮次关联与真实清理检查后清turn且转Idle，Idle重复完成幂等，返回Unit；不改StopAgent/Driver停止含义。
```

## 已实现的准备阶段收口

每条受理命令只解析一次并登记一个Other准备活动；审计及SQLite操作均在阻塞工作线程，真实join后mark_completed。GET包括纯alias也走数据库入口，INJECT不materialize资源定义，仅持久化ResourceID。effect的所有独立参数合成一次Get(expressions)，在一个读事务中读取，随后按启动时owned ImportBindings快照按ID与kind解析当前资源定义。只含alias/manual的effect不额外发送Block RPC。

MclAsyncPhase的Completed、PreparedEffect与PreparedImport避免将准备结果重新排回队首；准备结果携带pending.cancel，已取消的Prepared阶段不提交领域动作。已确认提交的CREATE/INJECT成功不因其后取消改成Cancelled。外部领域阶段继承同一取消token，避免准备与继续之间丢失来源取消。

停止或来源取消的未开始命令被移出FIFO，一批worker追加取消审计，MclCancelledOutput真实join后派发回执并减少cancel_audits。即使作用域已关闭不能登记新Other活动，drain仍等待这些cleanup-owned审计批次。合法history_append一次消息批次不附加outer command audit；类型或容器校验失败只记录指令审计。任何推理、工具、GET或INJECT结果都不隐式保存message。
