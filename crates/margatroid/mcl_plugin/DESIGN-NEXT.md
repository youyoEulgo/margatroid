# 伪代码格式

本文件采用DESIGN.md的伪代码格式：六板块按lib、system、handler、events、types、error组织；类型与函数按可见性分组，多步行为使用一个行为块并缩进。图书馆和资源类型仍在文档lib板块描述，共享事件与组件的源码按领域分别组织。

# 板块格式

```text
lib        插件实现与共享组件契约       Plugin及Agent命令队列的目标归属
system     src/system.rs             唯一MCL System的三阶段调度
handler    src/handler.rs            入队、异步结果收口、指令处理与解析
events     共享types/mcl_events.rs     命令、结果与内部异步契约
types      共享组件及解析辅助类型       FIFO队列、在途关联和类型化值
error      共享错误契约               MCL处理错误
```

**本文件只记录已确认的新设计，尚未实现。当前代码对应说明和解析规则保存在同目录DESIGN.md，不用本文件替代。**

# lib

## 类型

公开：
```text
MclPlugin：MCL命令插件，公开结构体
    schedule: String--唯一System所属运行计划
    impl Plugin for MclPlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                注册mcl_command_system
                使用共享Agent组件和WorldAgentExt访问数据，不依赖agent_plugin生命周期Handler
                异步操作通过通用异步闭包设施提交，不恢复多个领域请求、结果转发System
```

## 逻辑

```text
队列归属：
    Agent增加mcl: AgentMclQueue字段，定义归属共享types的agent_components.rs
    队列属于指令目标Agent，不属于来源；多个来源向同一目标发命令时进入同一FIFO
    这是待执行命令队列，不是LIFO调用栈，也不是Lua调用栈
    运行时Block仍在Agent.block中，队列不另存一份Block或资源内容
    Agent目标文档同步定义mcl字段、空队列初始化和AgentMclQueue/MclPending；本文件描述其处理流程

调用边界：
    Driver的注入mcl方法提交一条命令并等待该命令结果，等待期间Lua暂停
    单个Driver通常只有一条尚未完成的MCL调用，客户端可另外提交命令
    注入wait不是MCL指令，不进入队列、不占用MCL pending
    Driver等待时MCL System仍处理客户端命令，wake和ready无需因wait而绕过队列
```

# system

## 函数

crate公开：
```text
mcl_command_system(world: &mut World)
    命令System：crate公开System
    处理事件：MclCommand、Result<MclAsyncOutput, MclAsyncTaskError>
    产出事件：ClosureRequest、MclCommandResult
    行为：
        第一阶段克隆本帧全部MclCommand，逐条调用enqueue_mcl_command，只受理并入队
        第二阶段读取异步闭包结果，逐条调用complete_mcl_async，收口对应Agent的在途队首
        第三阶段遍历挂载Agent的存活Entity，逐个调用process_mcl_queue推进指令
```

## 逻辑

```text
三阶段顺序固定：
    先收新请求，再处理已完成异步结果，最后推进所有Agent
    第二阶段释放pending后，第三阶段可在同一帧继续处理该Agent后续命令
    同步操作处理和结果发送在当前帧完成，普通结果事件在后续事件帧才可被读取
    没有新事件但有已入队指令时，第三阶段仍可推进队列，不通过请求事件反复重投维持状态

并发：
    同Agent严格按队首顺序执行，异步未完成时不执行后续命令
    不同Agent独立推进，某Agent的异步等待不阻塞其他Agent
    不默认异步IMPORT需要队列后面的wake才能完成；当前已确认操作没有这种向后依赖
```

# handler

## 函数

crate公开：
```text
enqueue_mcl_command(world: &mut World, request: MclCommand)
    受理命令：crate公开函数
    行为：
        检查请求ID和来源、目标基本合法性，解析目标地址为唯一Agent图书馆Entity
        目标缺失或无Agent组件时立即发送复制原id、source、target的错误结果，不静默丢弃
        Creating或Running且Agent.execution全受理门开放才受理，Stopping或作用域关闭拒绝；同实例队列及pending已有该ID则返回DuplicateRequest，不入队
        将请求放入目标Agent.mcl.commands队尾，此阶段不执行指令
        source由注册Client入口或Driver宿主绑定，不能由外部任意伪造；来源失效不改写为新实例

complete_mcl_async(world: &mut World, output: Result<MclAsyncOutput, MclAsyncTaskError>)
    收口异步命令：crate公开函数
    行为：
        取得原请求、目标完整Entity和在途关联，核对Entity代数、pending和队首ID
        只有匹配本次在途队首才应用结果，不因旧结果弹出下一条或其他Agent命令
        必要的World回写在本阶段进行，异步闭包不能携带组件借用自行回写
        inference结果写入assistant历史，tool_call每项结果写入tool_call历史，包括失败；使用提交时的turn快照与agent_append_history_batch事务接口
        结果历史失败不覆盖真实执行结果，将错误放入history_error；不得因此重试工具或推理
        处理成功或失败后移除对应队首，清空pending
        发送原id、source、target原样复制的MclCommandResult
        重复结果不重复应用或回复，异常任务错误也必须有原请求关联

process_mcl_queue(world: &mut World, agent: Entity)
    推进目标队列：crate公开函数
    行为：
        先处理Stopping或execution全门关闭的队列取消及收口；随后pending非空时跳过新的执行
        pending为空时依次查看队首，捕获当前turn与原请求上下文并解析语法识别history_append
        除语法识别出的history_append外，先记录一次原始MCL指令及展开参数；写入失败弹出并回HistoryWriteFailed，不执行
        解析错误也记录原文后回错误，取消而未执行的排队请求记录取消审计；历史不可用仍必须回Cancelled并带history_error，不阻塞停止
        异步等待期间不重复记录指令，正常执行结果与停止清理独立，取消后已完成结果不改写为未执行
        解析失败时移除队首并发错误结果，继续下一条
        同步指令通过共享WorldAgentExt执行，完成后移除队首并发送结果，继续下一条
        异步指令先保存本次pending，再提交拥有所有权数据的异步闭包，保留队首
        提交异步后立即结束本Agent的本帧推进，让System继续下一个Agent
        闭包只读取、计算或等待，结果在第二阶段收口
```

## 函数

私有：
```text
parse_operation(instruction: &MclInstruction) -> Result<MclOperation, MclError>
    解析指令：私有函数
    行为：
        拒绝空指令和分号，规范化空白后按区分大小写的命令关键字分派
        CREATE BLOCK调用parse_create，GET调用parse_array_expression，INJECT调用parse_inject，EMIT EFFECT调用parse_effect
        IMPORT保留IMPORT <ResourceID> AS <alias>的严格四词语法，验证alias并拒绝额外arguments；alias保存在对应资源条目中，加载流程见begin_import
        MERGE、REF_MERGE及CREATE REF_BLOCK直接拒绝，不生成兼容操作
        BIND、LOAD、EXPOSE及未确定业务契约的旧入口不因旧代码存在而自动受理，迁移状态见本板块末尾
        语法解析不读取World，不检查字段是否存在，不提前求值选择器
        没有手动参数的指令拒绝额外arguments，不沿用旧selector分支静默忽略arguments的行为

normalize_command(command: &str) -> Result<String, MclError>
    规范化命令文本：私有函数
    行为：
        trim并折叠外部空白，允许方括号及逗号周围的空白，不改变标识符、数字符号或关键字大小写
        保留方括号、括号和逗号的结构边界，不用空白split代替数组表达式扫描
        括号配对及表达式合法性由具体解析函数检查，不把畸形输入修复为合法命令

validate_identifier(value: &str) -> Result<(), MclError>
    验证标识符：私有函数
    行为：首字节须ASCII字母或下划线，其余字节仅ASCII字母数字、下划线、点或连字符；拒绝空、点和双点，沿用当前标识符规则。

parse_create(command: &str) -> Result<MclOperation, MclError>
    解析普通Block声明：私有函数
    行为：
        要求CREATE BLOCK <block_id> (<字段声明>)，验证block_id
        字段声明为<inner_id> MESSAGE、RESOURCE或TOOL_CALL，分别创建AgentMessage、ResourceID、ToolCall的typed空数组
        验证所有字段ID，重复字段拒绝；不解析MERGE初始化或引用字段
        接受旧普通声明的空白及字段间逗号分隔，不接受未知字段类型
        匹配完整外层括号并拒绝闭括号后余文，不迁移旧解析器忽略尾部文本的漏洞
        空声明创建无字段Block；已有Block ID由执行阶段拒绝

parse_array_expression(text: &str) -> Result<MclArrayExpression, MclError>
    解析数组读取表达式：私有函数
    行为：
        按方括号外的逗号切分选择器，方括号内逗号保留为范围端点分隔
        每段trim后须非空；拒绝空表达式、前导逗号、尾逗号和连续分隔逗号
        按文本顺序调用parse_selector并设置Read位置，至少包含一个选择器
        不在此处检查数组元素类型、字段长度或范围越界
        问号只作为完整手动参数由外层解析，不可混入选择器列表

parse_selector(text: &str, position: MclSelectorPosition) -> Result<MclSelector, MclError>
    解析字段选择器：私有函数
    行为：
        按首个点分隔block_id和inner_id，分别验证标识符；路径不能为空
        无方括号为All，空方括号为Empty，一个整数为Single，两个逗号分隔端点为Range
        空端点为Open，非空端点解析为i64，超过两端或非法整数拒绝
        只允许一组完整方括号，拒绝嵌套、不配对及闭括号后余文
        保存原始范围与Read或Write位置，不在解析阶段按字段长度映射

parse_inject(command: &str, arguments: &[serde_json::Value]) -> Result<MclOperation, MclError>
    解析数组注入：私有函数
    行为：
        在括号结构外识别TO，左侧必须是一个完整数组来源，右侧必须是唯一目标选择器
        左侧选择器列表调用parse_array_expression；完整?生成Manual参数；完整[]生成Empty参数
        右侧调用parse_selector并设置Write位置，不接受多个目标、括号参数或问号
        选择器来源与[]拒绝额外arguments，?要求手动数组载荷
        不继续接受旧裸alias列表或单对象自动包装，资源数组使用ResourceID元素

parse_effect(command: &str) -> Result<MclOperation, MclError>
    解析效果参数列表：私有函数
    行为：
        要求EMIT EFFECT <effect_name>，按书写顺序扫描后续括号参数或独立问号
        每组括号内部调用parse_array_expression，括号之外不使用逗号分隔参数
        不允许嵌套括号、空括号、括号内问号、孤立闭括号或任意尾部词
        每个问号生成一个Manual参数，记录按出现顺序递增的绑定索引
        语法阶段保存数组参数顺序；执行前按具体effect契约检查参数数量和元素类型
        零参数effect不能携带多余括号或问号；旧FROM协议不自动兼容
        arguments每项必须为数组，数量等于问号数量，按出现顺序关联；一个问号也不省略外层

map_selector(selector: &MclSelector, length: usize) -> Result<MclStandardSelector, MclError>
    映射字段范围：私有函数
    行为：
        All映射[0,len]，Empty映射[0,0]，标准区间统一为左闭右开
        Single使用offset=len+1：非负Read为[x,x+1]，负Read为[offset+x-1,offset+x]
        非负Write为[x,x]，负Write为[offset+x,offset+x]，表示插入位置
        保留旧空字段Read[0]映射[0,0]的范围特例，但求值返回typed空数组而不是Unit
        Range的左Open取0、右Open取len，显式非负端点不变、负端点加offset
        双Open或两个显式端点混号拒绝，逆序或越界拒绝
        所有结果必须满足0<=start<=end<=len，整数计算检查溢出后再转usize

select_array(world: &World, agent: Entity, expression: &MclArrayExpression) -> Result<BlockInner, MclError>
    求值选择器数组：私有函数
    行为：
        核对完整目标Entity并取得Agent.block，仅读取普通Block
        依次读取每个字段的类型和长度，映射范围并复制区间值
        第一个字段确定结果类型，后续字段必须同类型；空区间也保留字段类型，不跳过类型检查
        按表达式顺序拼接数组，任一读取或类型检查失败则丢弃临时结果
        Single也返回typed数组，不产生单Message、Paths或空Unit，不建立后续自动更新关系

decode_manual_array(value: &serde_json::Value, kind: InnerType) -> Result<BlockInner, MclError>
    解码手动数组：私有函数
    行为：
        value必须为JSON数组，空对象、字符串和单消息对象都不能冒充数组
        Message数组逐项使用message_from_lua_json，Resource数组逐项解码ResourceID，ToolCall数组逐项按既有调用内容契约解码
        任一元素失败则整项参数失败，空数组按kind构造typed空值
        第一版不提供隐式alias展开或prompt字符串转消息，手动参数必须遵循三类数组元素契约
        此函数处理arguments的一项数组，不把整个arguments当作单项输入

message_from_lua_json(value: serde_json::Value) -> Result<AgentMessage, MclError>
    解码消息元素：私有函数
    行为：
        按共享AgentMessage契约解码inject、user、assistant、tool_call、error
        inject内部为AgentMessage数组，递归按同一契约解码，不额外限制其内部类型
        保留各消息的正文、调用ID、资源地址、usage等已有内容，具体字段以共享消息契约为准
        未知类型、非法资源地址或结构转换失败返回类型错误，不接受整个参数对象冒充数组

execute_sync_operation(world: &mut World, agent: Entity, operation: MclOperation) -> Result<MclValue, MclError>
    执行同步指令：私有函数
    行为：
        再次核对完整Entity与Agent，按操作分支执行，不重新按逻辑地址选取另一个实例
        CREATE准备全部typed空字段并检查重复，通过agent_operate_block创建Block；失败不覆盖已有Block
        GET调用select_array，返回类型化数组结果
        INJECT先读取目标字段类型和长度，求值全部左侧快照或解码手动数组，再检查目标范围与类型
        结束不可变借用后通过agent_operate_block进行一次范围替换；替换失败返回错误，不逐选择器写入
        INJECT保留原直接操作的Unit完成结果，左侧数组是输入求值结果，不等同于指令回执
        不预取所有prompt，不维护旧visible/default_visible、expose_mappings或state_bindings的自动刷新副作用
        IMPORT及异步effect由队列的异步分支处理，不在此函数伪装成同步成功

evaluate_effect_arguments(world: &World, agent: Entity, parameters: &[MclArrayParameter], manual_arrays: &[serde_json::Value], expected: &[InnerType]) -> Result<Vec<BlockInner>, MclError>
    求值效果数组参数：私有函数
    行为：
        具体effect提供有序参数类型契约，先检查参数数量和手动数组数量
        Selectors调用select_array，Manual按绑定索引调用decode_manual_array
        每项必须符合对应expected类型，不把相邻参数拼接为一个数组
        全部参数准备成功后才交给effect执行，失败不提交外部操作
        inference为消息数组、资源数组两项；tool_call为可见资源数组、工具调用数组两项
        history_append为AgentMessage数组一项，sandbox_use为ResourceID数组一项，wake/ready/start_turn/finish为零项
        异步提交拥有所有权的参数快照，不携带World、组件借用或会随Block修改的引用
```

## 函数

私有：
```text
begin_import(world: &mut World, request: &MclCommand, resource_id: ResourceID, alias: String) -> Result<(), MclError>
    发起资源导入：私有函数
    行为：
        核对目标完整Agent Entity、资源地址类型及alias，按资源类型选择加载请求
        为实际加载登记一次Other活动并移交加载请求，MCL不重复登记；创建oneshot，将Sender槽和活动派生cancel放入加载请求；Receiver移入MCL异步等待闭包，关联原请求和pending
        取消后加载方不再挂载材料并完成取消结果；等待闭包保留到加载真正收口，不以丢Receiver代替结束
        每次IMPORT都重新加载，不因集合已有ID而跳过；alias保存在新资源条目中
        加载插件异步读取并准备资源，在主线程完成ResourceSet对应键的插入或替换后发送oneshot
        加载失败保留旧条目；MCL收到成功或错误后完成本指令，释放队首
        公开加载Result事件可保留用于观察，不是MCL等待通道；AgentImage现有Workspace结果链不改
        resource_id必须精确属于Agent.info.image_dependencies，无builtin隐含例外；不恢复旧AgentResourceMap
        存储允许alias重名，但inference/tool_call的本次可见集合内冲突则准备失败

execute_sync_effect(world: &mut World, agent: Entity, name: &str, arguments: Vec<BlockInner>) -> Result<MclValue, MclError>
    执行同步效果：私有函数
    行为：
        wake为零参数，在短锁内操作共享wake槽位；closed返回错误，有Sender则取出发送，否则锁存pending_wake，返回Unit
        ready为零参数，核对本次启动槽位并取出Sender发送成功；没有Sender返回已交付就绪的信息，不重复发送
        ready槽位必须在Driver可调用前安装；没有Sender不证明Agent已进入Running，不等待启动结果事件
        sandbox_use为一个ResourceID数组，零项取消激活、一项检查并切换、多项报错且保持原值；未清理Sandbox活动阻止改变，Inference与Other不阻止
        start_turn为零参数，调用Agent.new_turn生成新代号并保存；finish为零参数，清空turn，已为空也成功
        history_append为一个AgentMessage数组，通过agent_append_history_batch在一个事务中追加，消息类型为inject/user/assistant/tool_call/error
        inject内部按AgentMessage数组处理，不施加比AgentMessage契约更窄的白名单
        这些操作不为通知或字段修改创建异步pending；history_append不额外生成自己的MCL指令记录

begin_inference(world: &mut World, request: &MclCommand, messages: Vec<AgentMessage>, resources: Vec<ResourceID>) -> Result<(), MclError>
    发起推理：私有函数
    行为：
        参数依次为消息数组与可见资源ID数组，捕获当前turn快照及原请求关联
        内容沿用既有推理消息与模型配置契约，不在MCL展开InferencePlugin内部行为
        创建oneshot并发送推理请求，Sender交给执行方，Receiver移入MCL异步闭包
        推理设施为实际HTTP请求登记一次Inference活动，工具设施登记Sandbox或skill的Other活动，MCL只持逻辑取消关联；请求使用Agent.execution派生取消令牌，异步闭包等待完整InferenceResponse，保留message及stop_reason，通道关闭转关联错误
        主线程收口时自动追加assistant历史，再完成MclCommandResult；不重复记录已写的MCL指令
        正常完成产生一条MCL指令记录和一条assistant记录；失败不伪造assistant成功消息

begin_tool_call(world: &mut World, request: &MclCommand, visible: Vec<ResourceID>, calls: Vec<ToolCall>) -> Result<(), MclError>
    发起工具调用批次：私有函数
    行为：
        参数依次为可见资源ID数组与ToolCall数组，先验证调用结构、必需字段和调用ID唯一性
        遍历目标Agent的可调用资源集合，只挑选map key在visible数组中的条目
        对每个调用比较tool_name与条目get_alias()，找到匹配即为可用；不依赖全局alias表或旧推理pending
        本次可见的可调用集合出现重名alias时整个批次准备失败，不按遍历顺序任选；没有匹配填该调用失败模板
        结果槽位按原calls顺序建立，可用调用各创建一个oneshot并发送独立执行请求
        MCL异步闭包收集所有Receiver的结果并放回原槽位；单项失败或通道关闭填该项失败模板，不丢弃其他调用
        没有可执行调用时直接使用已填好的失败结果，不为等待不存在的Receiver阻塞队列
        主线程收口逐项追加tool_call历史，包括可见性拒绝、执行失败和成功结果，使用提交时turn快照
        按原调用顺序返回AgentMessage结果数组并完成MclCommandResult；N个调用正常收口为N条tool_call记录加一条MCL指令记录

record_mcl_command(world: &World, agent: Entity, request: &MclCommand, turn: Option<String>) -> Result<(), MclError>
    自动记录指令：私有函数
    行为：
        除history_append外每个队首请求只记录一次，保存原始指令而非normalize后的文本
        使用问号时按其参数位置展开实际数组并保存，不把仅含问号的模板当作完整历史
        保留原文本及结构化展开参数以避免字符串替换破坏数组内容，记录source、请求关联及turn快照
        语法或参数错误也不能无声丢失已受理指令，无法展开时保留原文、载荷及错误信息
        指令记录失败返回HistoryWriteFailed并不开始执行；结果历史失败保留真实执行result，单独填写history_error，不自动重做外部操作
        使用完整Agent Entity、命令ID、记录类别和调用ID或数组index构造幂等键；批次历史在单事务中追加，重复键相同内容不重复写，冲突内容报错
```

## 逻辑

```text
同步通知时序：
    帧n执行wake或ready时立即交付Sender并发MclCommandResult；不等待接收任务实际运行
    通知Receiver所在执行任务或启动闭包由执行线程独立调度，不能保证具体在哪一帧继续
    MclCommandResult在后续事件帧被来源接收，确认通知动作完成，不证明对方已完成后续工作
    ready通知与Driver等待MCL结果是两条独立链路，启动闭包可先继续而Driver随后收到指令结果

异步与历史：
    IMPORT、inference、tool_call使用oneshot作为外部一次性完成通道，统一由MCL异步闭包等待
    框架Result用于闭包到ECS的收口，不与外部oneshot或公开观察事件混为一条通道
    指令历史仅记录一次；结果历史在收口且关联匹配后追加，重复结果不重复记录
    turn与执行参数在提交时取快照，不将旧任务结果归入后来生成的新turn
    删除两个visibility来源effect，显式资源数组仅决定本次调用可见性，不建立持久来源绑定
```

## 逻辑

```text
解析与执行顺序：
    队首请求 -> parse_operation -> 类型化操作
    同步操作 -> 字段读取与范围/类型检查 -> 一次机械修改或数组结果 -> 移除队首并发送结果
    异步effect -> 参数快照 -> 保存pending并提交 -> 后续结果回写 -> 移除队首并发送结果
    选择器只在队首实际执行时求值，不在请求入队时冻结字段内容

范围示例：
    len=4时读取[-1]为[3,4]，读取[-3,-1]为[2,4]
    写入[-1]为[4,4]尾部插入，写入[0]为[0,0]头部插入
    INJECT [] TO b.f替换[0,len]，因此清空字段；INJECT [] TO b.f[]替换空区间，不清空全字段
    GET b.f[]返回对应字段类型的空数组，GET空字段b.f[0]同样返回typed空数组

迁移边界：
    已展开：普通CREATE、GET、多选择器求值、INJECT、effect参数解析、索引范围和手动数组/消息转换
    IMPORT、inference、tool_call通过oneshot等待外部完成；不恢复旧全局pending表
    ready、wake、sandbox_use、start_turn、finish为同步effect；history_append通过共享历史追加接口完成
    删除history_record、visibility_source、default_visibility_source，不生成兼容操作
    第一版不受理EXPOSE，不恢复AgentResourceMap或refresh_exposed；若需要再设计独立协议
    BIND/LOAD STATE不恢复实时数据库及state_bindings；当前代码参考保留旧行为，但目标执行路径未受理
    当前实现的非事务副作用不照搬，新INJECT只保证预读检查及一次字段替换，不宣称跨异步操作全局事务
```

## 逻辑

```text
同步与异步：
    GET、INJECT、CREATE及其他纯内存机械操作同步处理
    异步IMPORT读取加载或其他外部操作的完成来自对应设施，不需要Lua继续执行才能返回
    ready取用Agent.lua.ready并交付就绪，wake取用共享wake槽位并发送或锁存通知
    ready和wake完成的是通知动作，不等待启动结果事件或被唤醒Driver继续运行
    inference与tool_call提交携带oneshot Sender的请求，MCL异步闭包等待Receiver；外部插件内部执行另行设计

新语法与当前参考的边界：
    取消全部引用Block能力：CREATE REF_BLOCK、REF_MERGE、RefBlock、RefMerge及引用存储不进入目标设计
    取消MERGE指令和CREATE中的MERGE声明，以多选择器数组读取表达式替代，不建立持久引用
    ResourceID仍是资源地址值，不因取消引用Block而删除资源标识
    从当前DESIGN.md保留命令规范化、标识符验证、普通CREATE字段声明及选择器索引、范围检查的可用逻辑
    GET单索引改为数组返回，不继续沿用旧单Message或空Unit特例
    保留消息和资源绑定值的类型化转换能力，但effect输入统一改为数组参数
    第一版不受理BIND/LOAD STATE，不恢复实时数据库和state_bindings
    当前DESIGN.md继续记录实际旧代码，不把上述目标语法写成已实现

数组读取表达式：
    GET a.xxx[x], b.xxx, c.xxx[x,y]
        按书写顺序读取各选择器，拼接为一个同类型数组，不写来源、不建立引用
        单索引返回单元素数组，空选择返回空数组；不因元素数改变返回形状
        只在方括号外的逗号分隔选择器，[x,y]内的逗号属于范围端点
        不同元素类型不能拼接；空数组的类型由字段或操作参数上下文确定

INJECT左侧与目标：
    INJECT a.xxx[x], b.xxx, c.xxx[x,y] TO d.xxx[start,end]
        左侧与GET使用同一数组读取表达式；右侧是唯一可写普通字段的目标范围
        先读取所有左侧选择器为快照并检查类型、目标范围，再进行一次范围替换
        来源和目标为同字段时也不能边读边写，后续来源不读取本次修改后的值
        保留目标All、空范围及单索引插入的现有范围映射，右侧不接受多个目标选择器
    INJECT ? TO d.xxx[start,end]
        手动传入一个数组作为左侧值，不接受单元素对象自动包装

Effect数组参数：
    EMIT EFFECT inference (a.messages) (b.resources)
        每个括号是一项独立数组参数，参数次序和数量由对应effect契约决定
        括号内部允许与GET相同的多选择器表达式，按顺序拼接成该参数数组
    EMIT EFFECT inference (a.messages, b.messages) (c.resources)
        第一个参数是拼接后的消息数组，第二个参数是资源数组，两者不互相拼接
    EMIT EFFECT inference ? (b.resources)
        一个?替代一个完整括号参数，从手动绑定数据取得一个数组
    EMIT EFFECT inference ? ?
        两个?分别代表两个手动数组参数，不是把一个数组拆成两项参数
        括号参数与?可以混用；括号中不接受?，?不表示单个数组元素
        手动数组按?出现顺序对应，不接受对象或标量替代数组；绑定数量和元素类型必须匹配
        零参数effect不写括号或?，不强制传空数组
        effect输入统一为数组参数，结果仍可为Unit或其他类型化结果
        不新增分号参数分隔语法，不沿用inference FROM RefBlock或旧FROM ?对象协议
        command.arguments统一为数组参数列表，每项为一个手动数组；无问号为空，不允许单问号省略外层
        tool_call接受可见ResourceID数组和ToolCall数组；history_append接受AgentMessage数组，不保留history_record

队列调度与停止策略：
    每Agent每帧使用有界同步指令预算，默认64条，队列有剩余则下帧继续，所有Agent每帧都获推进机会
    Creating和Running且作用域开放可受理，Stopping或execution全门关闭拒绝新命令；取消未执行排队项并回Cancelled，取消在途令牌但保留关联直到真实收口
    Driver正常结束也关闭execution全门，System每帧第三阶段检查该门，无需新增结束或停止协调事件
    第三阶段即使pending非空也先处理停止收口：排队尾部一次回复后移除，pending队首等待外部结束及MCL结果历史/回执完成
    Agent.mcl.drained通知只在队列为空且pending为空时完成；Driver清理等待该通知，不阻塞ECS
    等待错误不能只记录日志，队首必须完成或明确取消，否则后续命令永久停住
    同一完整Agent实例的排队及在途ID唯一，重复返回DuplicateRequest；已完成ID不设永久去重表，调用方必须生成新ID
```

# events

## 类型

公开，统一定义于共享types/mcl_events.rs；插件不保留events.rs：
```text
MclCommand：MCL指令请求，公开事件
    id: String--请求关联ID
    source: ResourceID--原始来源地址
    target: ResourceID--目标Agent地址
    command: MclInstruction--指令内容
    impl Event for MclCommand

MclCommandResult：MCL指令结果，公开事件
    id: String--原请求ID
    source: ResourceID--原请求来源，直接复制不交换
    target: ResourceID--原请求目标，直接复制不交换
    result: Result<MclValue, MclError>--真实执行成功结果或错误，不因执行后历史失败改写
    history_error: Option<HistoryError>--执行完成后的附带历史错误，无错误为None
    impl Event for MclCommandResult
```

## 逻辑

```text
请求与结果：
    id区分同一source与target之间的多条请求，不能只按来源目标或完成顺序猜测
    source和target在结果中保持原值，结果消费者自行按id与source接收
    source失效不允许把旧请求改写成新实例来源
    内部异步Result的输出和关联错误也由共享mcl_events.rs定义，不引用插件私有类型
    公共自动Result、ClosureRequest与公开MclCommandResult是不同通道
```

# types

## 类型

公开共享契约：
```text
AgentMclQueue：目标Agent的命令队列，公开类型--定义于共享agent_components.rs，Agent组件的一个字段，不是独立Resource
    commands: VecDeque<MclCommand>--FIFO队列，异步在途命令留在队首
    pending: Option<MclPending>--当前异步队首的关联，不是其他Agent的全局等待表
    drained: AgentMclDrainHandle--队列及pending为空且回执完成的共享通知
    accepting: bool--主线程入队门，作用域关闭时停止受理

MclPending：异步在途关联，公开类型--定义于共享agent_components.rs
    command_id: String--队首请求ID
    target_entity: Entity--提交时的完整目标Entity
    operation_id: String--本次异步操作唯一关联，防止重复或迟到结果完成另一操作
    cancel: CancellationToken--本次指令派生取消能力，Stopping时取消，真实收口后清除

MclInstruction：指令内容，公开类型--指令文本与可选绑定数据
    text: String--原指令文本
    arguments: Vec<serde_json::Value>--每项必须为数组，依次对应一个问号；没有问号为空
    说明：统一参数封装，不论一个或多个问号均保留外层arguments数组，不增顶层请求字段

AgentMessage：目标消息值契约，公开共享类型--不是旧Agent消息路由事件
    inject(Vec<AgentMessage>)--注入消息数组，元素仍为AgentMessage
    user--用户消息
    assistant--推理结果，保留既有内容及usage
    tool_call--工具结果，含调用关联、资源地址及失败信息
    error--错误消息
    说明：准确字段以共享types目标文档为准；旧同名路由事件删除，不作为消息值继续使用

推理与工具执行回复：公开共享事件契约
    说明：推理请求及每条工具执行请求携带一次性oneshot完成能力，载荷定义于所属领域的xxx_events.rs
        请求允许Clone时共享同一个Arc<Mutex<Option<oneshot::Sender<Result<输出, 错误>>>>>回复槽，不复制Sender
        执行方一次性取出Sender发送成功或失败，MCL异步闭包独占对应Receiver
        不另加MCL专用全局等待注册表；通道关闭须转成关联错误或单项工具失败

MclValue：MCL结果值，公开类型
    Unit
    Array(BlockInner)
    ResourceImport { resource_id: ResourceID, alias: String }
    Inference(InferenceResponse)
    Text(String)--ready已交付等同步信息
    说明：工具批次返回Array(Message)，无Paths或GET单消息特例，推理不丢弃stop_reason

MclAsyncOutput：异步命令输出，公开共享类型--定义于mcl_events.rs
    request: MclCommand--原请求上下文
    pending: MclPending--完整目标与操作关联
    result: Result<MclAsyncPreparedResult, MclError>--oneshot等待完成的结果或关联错误
    turn: Option<String>--提交时轮次快照，用于自动结果历史

MclAsyncPreparedResult：异步准备结果，公开共享类型
    Import--加载完成回执，资源集合已由加载插件插入
    Inference(InferenceResponse)--assistant、usage及停止原因完整返回
    ToolCall(Vec<AgentMessage>)--按原调用顺序排列，含失败项
    说明：只携带拥有所有权数据，用于主线程结果Handler记录历史和发送回执，不携带World借用

MclAsyncTaskError：可关联的异步监督错误，公开共享契约
    说明：满足From<AsyncTaskError>，但该From不会自动获得请求ID；提交包装必须保障失败结果仍能关联请求并释放pending
```

## 类型

私有，解析辅助类型留在插件types，不作为跨插件事件契约：
```text
MclOperation：解析后操作，私有枚举
    CreateBlock { block_id: String, fields: Vec<BlockFieldDeclaration> }
    Get { expression: MclArrayExpression }
    Inject { source: MclArrayParameter, target: MclSelector }
    Import { resource_id: ResourceID, alias: String }
    Effect { name: String, parameters: Vec<MclArrayParameter> }
    说明：没有RefBlock、Merge、RefMerge变体；业务未确定的旧入口不生成可执行变体

BlockFieldDeclaration：空字段声明，私有结构体
    inner_id: String--字段名
    kind: InnerType--Message、ResourceID或ToolCall三类数组，与共享Block契约一致

MclArrayExpression：数组读取表达式，私有结构体
    selectors: Vec<MclSelector>--非空有序列表，列表中只能为Read选择器

MclArrayParameter：数组输入，私有枚举
    Selectors(MclArrayExpression)
    Manual { index: usize }--问号对应的手动数组索引
    Empty--仅用于INJECT []，按目标字段类型构造空数组
    说明：effect不解析Empty，空手动数组通过?传入；不增加内联JSON文本语法

MclSelector：原始字段选择器，私有结构体
    path: BlockPath--普通Block字段路径
    range: MclRange--未按长度映射的范围
    position: MclSelectorPosition--读取或写入位置

MclSelectorPosition：选择器位置，私有枚举
    Read
    Write

MclRange：原始选择范围，私有枚举
    All
    Empty
    Single(i64)
    Range(MclEndpoint, MclEndpoint)

MclEndpoint：范围端点，私有枚举
    Value(i64)
    Open

MclStandardSelector：标准字段区间，私有结构体
    path: BlockPath--目标路径
    start: usize--含起点
    end: usize--不含终点
```

## 逻辑

```text
不复制数据：
    队列保存请求与在途关联，Block、资源集合和历史句柄仍各自只有一份事实来源
    Source、target公开使用ResourceID，执行和回写内部必须核对目标完整Entity
    不新增全局PendingMclImports或PendingMclEffects替代Agent队列
```

# error

## 已确认的外部异常链路

```text
agent_exception_system
    处理事件：DriverLuaVmExceptionResult、AgentException
    产出事件：AgentExceptionResult

workspace_exception_system
    处理事件：AgentExceptionResult、WorkspaceException
    产出事件：WorkspaceExceptionResult

上述领域异常接口同步到Agent和Workspace目标文档，不替代MCL请求结果
本文件只记录接口，不代替其他插件定义异常处理策略
MCL指令错误结果不能被异常通知替代，源请求仍需对应MclCommandResult
```

## 逻辑

```text
错误定义：
    当前MclError和解析错误对应说明以DESIGN.md为准，不凭空删掉现有错误分类
    目标设计需补齐目标失效、重复请求、异步取消和队列关闭等错误，具体分类后续确认
    普通解析或执行失败返回对应请求结果，不自动终止整个Driver或Agent
    何种错误需要另发领域异常事件后续明确，不能用异常事件替代指令回执
```
