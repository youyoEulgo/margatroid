# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名和直接行为。
System区分事件、闭包结果、提交闭包和oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
方法使用下列层级；--仅承载简短说明，执行逻辑写入行为。
method(args) -> xxx
    行为：
        xxx
```

# 板块定义

```text
# lib        mcl/core/src/lib.rs；mcl/plugin/src/lib.rs  解析入口与队列扩展；Plugin安装
# components mcl/core/src/components.rs                MclQueue
# events     mcl/core/src/events.rs                    MclCommand
# system     mcl/plugin/src/system.rs                  请求受理、队列分派与执行锁
# handler    mcl/plugin/src/handler.rs                 参数求值及领域调用
# types      mcl/plugin/src/types.rs                   执行上下文和异步载荷
# types pub  mcl/core/src/types.rs                     指令、语法树、选择器与结果值
# error      mcl/core/src/error.rs                     MclError
```

# lib

## 函数

```text
-- 公开，定义于mcl_core
parse_instruction(instruction: &MclInstruction) -> Result<MclOperation, MclError>
    行为：
        按IMPORT、CREATE、GET、INJECT、CONFIG、EFFECT分流；
        检查语法和问号参数数量，不访问World或文件。
```

## 类型

```text
-- 公开，定义于mcl_core
WorldMclExt：队列访问扩展，公开trait
    mcl_close(&mut self, target: Entity) -> Result<(), MclError>
        行为：
            关闭入队门，取消在途命令，排队项转入取消收口。
    mcl_drain(&self, target: Entity) -> Result<MclDrain, MclError>
        行为：
            取得共享收口等待能力。
    impl WorldMclExt for World
```

```text
-- 公开，定义于mcl_plugin
MclPlugin：MCL执行插件
    schedule: String
    impl Plugin for MclPlugin
        build(self, app: &mut App)
            行为：
                检查运行设施，schedule仅允许UPDATE或POST_UPDATE，拒绝PRE_UPDATE及无法证明顺序的自定义schedule，安装mcl_command_system，不提供Lua宿主函数。
```

# components

## 类型

```text
-- 公开
MclQueue：目标Entity的命令队列，公开组件
    commands: VecDeque<MclCommand>
    locked: Arc<AtomicBool>--跨线程指令锁；Acquire读取，Release解锁
    active: Arc<Mutex<BTreeMap<String,MclReply>>>--实际在途id及原回复身份，不是返回队列
    accepting: bool
    drain: MclDrain
    cancel: CancellationToken
    default() -> Self
        行为：同步创建空命令和active集合、未关闭cancel及接受门，不要求Driver VM或文件资源。
    impl Component for MclQueue
```

# events

## 类型

```text
-- 公开
MclCommand：命令请求，公开事件
    id: String
    source: Entity--由网络或共享Lua宿主绑定的真实来源
    source_address: String--调用入口必需显式提供自身真实地址；校验与source一致，MCL审计source不记录Entity编号；history_append消息无source
    source_instance: Option<String>--自动Lua来源为执行start_id，工具来源为唯一task-instance，客户端为None；不引用Lua core类型
    target: Entity
    instruction: MclInstruction
    cancel: CancellationToken
    reply: Arc<Mutex<Option<oneshot::Sender<Result<MclValue, MclError>>>>>
    impl Event for MclCommand
```

# system

## 函数

```text
-- crate公开
mcl_command_system(world: &mut World)
    处理事件：MclCommand
    提交闭包：execute_command(CommandContext) -> Future<Output = ()>
    产出事件：None
    oneshot回复：Result<MclValue, MclError>
    行为：
        MclCommand -> handle_command；
        MclQueue -> handle_queue。
```

# handler

## 函数

```text
-- crate公开
handle_command(world: &mut World, request: MclCommand)
    行为：
        核对来源和目标完整Entity、实际Lua执行实例、入队门及重复id；目标统一仅Starting/Running可接受，Closing拒绝，不按具体指令增加状态或turn受理条件。
        入目标FIFO；
        拒绝通过reply返回错误。

handle_queue(world: &mut World, target: Entity)
    行为：
        目标locked为true时跳过；否则循环pop_front，取消/无效来源/语法错误直接原oneshot回复并drain.completed，继续下一条；
        工具调用、推理及等待推理收口的finish_turn不持队列锁，其他操作先store(true)再提交异步闭包；
        分派时登记active[id]=原reply、取得目标Session/turn/Prompt快照及CommandCompletion，构造owned CommandContext；
        调用既有AsyncRuntimeHandle::spawn提交无返回事件的owned执行Future；不锁操作发出后继续分派下一条，锁操作发出后停止处理该队列；真实完成解锁时WorldClosureSender发送一次空闭包唤醒所属执行器；
        不处理MCL返回事件，不在队列保留已分派命令，不实现隔离或多指令事务。
```

```text
-- 私有
execute_command(context: CommandContext) -> Future<Output = ()>
    行为：
        执行execute_operation并等待真实收口，取消时通知执行能力后继续等待，不以丢弃Future结束操作；
        执行完成后独立追加一次MCL审计，记录来源、实例、指令、参数及真实执行结果；
        审计失败返回说明执行可能已提交的错误，不能伪称执行未发生；
        结果交context.completion.finish：移除active，持锁者解锁，直接消费原oneshot回复，然后完成drain计数，并唤醒执行器；不发MCL完成事件；
        不跨await借用World，不重复解析或审计。

execute_import(context: CommandContext, kind: ImportKind, locator: String, alias: String)
    行为：
        IMPORT TOOL校验资源后向Agent的Tools登记alias、description、input_schema和path；
        其余IMPORT按资源类别登记，沙箱使用由对应effect处理。

read_expression(context: &CommandContext, expression: MessageExpression) -> Future<Output = Result<Vec<serde_json::Value>, MclError>>
    行为：
        调用prepare_sources取得有序消息来源，发送ReadMessageSources并等待回复；
        校验返回的消息数组，非Messages结果返回StorageFailed。

prepare_sources(context: &CommandContext, expression: MessageExpression) -> Future<Output = Result<Vec<storage_core::MessageSource>, MclError>>
    行为：
        Selector转换为存储选择器，携带目标Agent.instance；instance每次实体创建唯一，不再依赖AgentSession组件；
        PromptAlias按已导入路径在blocking worker读取，按资源角色构造Literal来源；
        保留来源书写顺序，不在此改写目标Block。

execute_operation(context: &CommandContext) -> Future<Output = Result<MclValue, MclError>>
    行为：
        执行前检查命令及执行取消标记，按MclOperation分派；
        Create转换CreateBlock，TEMP绑定目标Agent.instance；Get调用read_expression并返回Array.Message；
        Inject将手工数组、空数组或表达式转换为消息来源，交存储ReplaceMessages完成替换，等待结果返回Unit；
        Config转换ReadConfig、SetConfig或DeleteConfig，仅访问当前Agent设置，读取返回Config，修改确认后返回Unit；
        Import及Effect分别调用execute_import和execute_effect，不在本函数回复原请求或写命令审计。

execute_effect(context: &CommandContext, name: &str, arguments: Vec<EffectParameter>) -> Future<Output = Result<MclValue, MclError>>
    行为：
        按effect参数约束求值，通过对应领域core能力执行或发送带oneshot的具体领域请求；
        保留请求显式提供的可选回合关联。inference使用消息及推理参数，工具调用使用完整调用数组；省略turn保持None，不抓取目标当前turn自动绑定。
        工具定义与查找来自目标Agent的Tools。中止由队列外的控制流程受理，各System复检turn。
        按name分派领域调用并将参数求值为该领域输入，等待结果后返回MclValue或MclError；
        同步存储能力在blocking worker执行并等待真实join；不在此函数发送原请求reply或写入统一命令审计；
        ready转交Agent core的agent_ready；start_turn调用agent_start_turn并返回MclValue::Turn(String)，其值是实际当前turn；finish_turn同样调用Agent领域方法，不在MCL重复判断状态或turn策略。
        不提供agent_stop effect，关闭由Lua agent.stop注入或StopAgent领域请求处理。
        model_set校验InferenceParameters及真实ModelRoutes，首次安装从inference_core::ModelObserverFactory.for_owner取得通用回调，再使用AgentInferenceConfig统一入口安装/更新，返回安全模型元数据Config；不读取Driver manifest。
        history_append取得目标owned AgentStorage，复检命令及取消，在worker调用append_message_history并返回Array.Message。

finish_turn(context: &CommandContext) -> Future<Output = Result<MclValue, MclError>>
    行为：仅经通用check_action复检来源、MCL状态门及取消；只克隆与待结束turn明确关联的InferenceHandle并在World外等待wait_finished真实退出；自由推理句柄不属于该回合收尾范围。随后调用Agent core的agent_finish_turn，由Agent领域验证Running/turn关联并清turn；成功只移除已等待的同request_id、同turn且真实结束的句柄，不能移除并发出现的另一请求；MCL不重复实现回合受理规则。
        不因正常后台收口返回要求Lua重试的错误，不等待已Return后台工具；实际清理失败才返回CleanupFailed。
block_effect_argument(context: &CommandContext, parameter: &EffectParameter) -> Result<&serde_json::Value, MclError>
    行为：取得Manual指定的原始JSON参数，缺失或非Manual返回ArgumentMismatch，不作为消息数组解释。

execute_block_effect(context: &CommandContext, name: &str, parameters: Vec<EffectParameter>) -> Future<Output = Result<MclValue, MclError>>
    行为：
        按既有标识符规则校验Block名，恢复参数校验为SQLite范围内正整数；不接收物理路径
        通过World闭包check_action复检命令、来源及取消，克隆目标AgentStorage，不跨await借用World
        owned句柄及目标Agent.instance命名空间交给blocking worker，提交前复检取消，执行对应存储能力并await真实join
        将存储结果转换为Unit或BlockSnapshots，错误沿storage_error转换，worker失败为CleanupFailed
        不在本函数写审计或发送原请求reply，不自动重试

explicit_turn(context: &CommandContext, parameter: Option<EffectParameter>) -> Result<Option<String>, MclError>
    行为：省略返回None；明确参数只接受Manual指定的非空字符串，读取对应原始JSON值，不从Agent状态或执行上下文补齐。parse_instruction先验证第二参数类型、长度与控制字符。

execute_inference(context: &CommandContext, messages: Vec<Value>, turn: Option<String>) -> Future<Output = Result<MclValue, MclError>>
    行为：compact输入在inference_message中按assistant别名规范化，保留相同reasoning/content/tool_calls并映射为InferenceMessage::Assistant；不把compact角色发往厂商。转换消息与读取实际推理配置，经通用check_action后提交InferenceRequest；turn只来自本次effect明确提供的可选参数，不来自Agent当前状态快照；省略为None，空字符串或非字符串明确报参数错误。
        无turn也正常提交，不在MCL构造empty_completed或短路返回；Inference plugin决定生命周期、会话、权限及并发受理，Some(turn)额外要求当前有效且匹配，None为自由调用，并以原oneshot返回结果。
        InferenceRequest的领域Err转换为MclValue::Inference { message: {type:"error",message:原领域Display文本,request_id:原MCL id}, stop_reason:"error" }，属于成功交付的推理错误消息，不转换顶层MclValue::Error或抛Lua异常。
        正常响应仍由inference_result转换assistant；真实MCL参数/身份/权限/取消、配置缺失及oneshot丢失等执行通道错误保留原MclError，不伪造正常返回或自动重试。

execute_tools(context: &CommandContext, messages: Vec<Value>, turn: Option<String>) -> Future<Output = Result<MclValue, MclError>>
    行为：
        不判断Agent是否Running、有无turn或工具alias存在性；按通用MCL门通过后，合法调用全部发给ToolExecuteRequest；显式可选turn原样透传，省略为None，不继承当前Agent或调用工具的turn。
        Tool plugin统一验证Starting/Running、实际会话、权限、alias/Schema及任务受理；Some(turn)额外要求Running且当前turn匹配，None不要求目标有turn，MCL只收齐并转换领域结果。
        验证调用身份后，一次性发出多个ToolExecuteRequest，各自携带Agent、原turn、call_id及独立oneshot；
        工具System按请求名称查当前Tools，存在则调用，不存在则返回对应工具错误。所有请求先发出，再用join_all收齐并按输入顺序返回。
        中止前未被工具System受理且显式携带该turn的请求直接回中止结果；None自由调用不属于该回合。
        已经受理、明确绑定该turn且未return的任务由ToolTasks强制return并清理，使本闭包立即获得各调用的中止结果，不再等工具自然return。中止获胜后迟到Return无法再次交付，正常return先获胜的结果保留且后台任务不被杀。
        同一Lua资源不同alias仍各自创建独立任务，MCL不按path或owner将它们串行化；
        A/B之间的请求路由及通信失败处理属于Lua工具内部协议；Tools记录删除不取消已进入ToolTasks的调用。
        可识别工具调用错误始终返回对应failed工具消息；
        整批收齐关联后交Driver，Driver显式结束回合。不得通过关闭整条MCL队列或杀Driver来中止。
```

# types

## 类型

```text
-- 私有
CommandContext：执行材料--原MclCommand、MclOperation、owned存储与执行能力、路径和取消能力
    completion: CommandCompletion--执行闭包持有的收尾guard
CommandCompletion：执行收尾guard
    request: MclCommand
    active: Arc<Mutex<BTreeMap<String,MclReply>>>
    locked: Arc<AtomicBool>
    owns_lock: bool
    drain: MclDrain
    world: WorldClosureSender
    settled: bool
    finish(&mut self,result:Result<MclValue,MclError>)
        行为：至多一次；记录CleanupFailed/CommitUnknown供drain，移除active，持锁者Release解锁，通过原reply回复后drain.completed；持锁者再提交一次World空闭包触发事件驱动执行器处理排队请求。
    Drop
        行为：没有确认结果则以CleanupFailed收尾，不留下锁或丢失原reply。
check_action(world,request,cancel)
    行为：复检取消及来源、目标受理门，active必须含同id且Arc::ptr_eq匹配原reply；不要求命令仍位于队首。
```

# types pub

## 类型

```text

-- 公开
MclInstruction：输入指令
    text: String
    arguments: Vec<serde_json::Value>--按问号位置传入，配置参数不强制数组

MclOperation：语法树，公开枚举
    Import { kind: ImportKind, locator: String, alias: String }
    Create { name: String, fields: Vec<String>, temporary: bool }
    Get(MessageExpression)
    Inject { source: MessageParameter, target: Selector }
    Config(ConfigOperation)
    Effect { name: String, arguments: Vec<EffectParameter> }

ImportKind：文件资源种类，公开枚举
    Message | Tool | Sandbox

ConfigOperation：设置操作，公开枚举
    Get
    Set { path: String, value: toml::Value }
    Delete { path: String }

Selector：消息字段选择器
    block: String
    field: String
    range: Selection

Selection：选择范围，公开枚举
    All
    Index(i64)
    Range(i64, i64)

MessageExpression：有序消息来源
    sources: Vec<MessageSource>

MessageSource：消息来源，公开枚举
    PromptAlias(String)
    Selector(Selector)

MessageParameter：消息参数，公开枚举
    Expression(MessageExpression)
    Manual(usize)
    Empty

EffectParameter：效果参数，公开枚举
    ToolCalls(MessageExpression)--只允许tool_call的字段投影，原Storage选择单消息后取得tool_calls
    Expression(MessageExpression)
    Manual(usize)

MclValue：命令返回值，公开枚举
    Unit
    Turn(String)--start_turn返回的实际当前回合，Lua投影为{Turn=turn}，wire为data.kind=turn/value字符串
    Error(serde_json::Value)--type=error、message及request_id；唯一回复出口将非工具错误转换为此值
    Array { Message: Vec<serde_json::Value> }
    Config(toml::Table)
    BlockSnapshots(serde_json::Value)--指定Block的timeline与已提交快照映射，不含其他Block
    Inference { message: serde_json::Value, stop_reason: String }
    Import { kind: ImportKind, alias: String }

MclDrain：线程安全收口能力
    wait(&self) -> Future<Output = Result<(), MclError>>
        行为：
            等待outstanding归零后才返回已记录清理错误或成功；不会因一个在途请求先失败而略过其他尚未完成请求。
```

# error

## 类型

```text
-- 公开
MclError：命令错误
    kind: MclErrorKind
    message: String
    impl Display + std::error::Error

MclErrorKind：错误类别，公开枚举
    ParseFailed | ArgumentMismatch | SourceInvalid | TargetMissing | Closed
    AliasConflict | AliasMissing | ImportDenied | InvalidMessage | InvalidRange
    ConfigPathInvalid | ConfigTypeMismatch | StorageFailed | CommitUnknown
    CleanupFailed | EffectFailed | Cancelled | ReplyClosed
```

# 持有关系

```text
Agent Entity
├── Agent（agent_core）
├── MclQueue（mcl_core）
├── 数据库及设置路径（所属领域）
└── 各资源alias到路径的集合（所属领域core）

共享LuaServices的mcl_execute服务
├── 真实source与实际Lua执行实例关联
└── MclCommand.reply接收端

mecs全局异步设施
└── execute_command
    ├── owned执行能力
    └── 存储及领域请求的oneshot接收端
```
