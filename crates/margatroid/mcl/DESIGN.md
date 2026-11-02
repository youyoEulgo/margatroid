# 伪代码格式

```text
板块使用一级标题，一行注明core与plugin文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名和直接行为。
System区分事件、闭包结果、提交闭包和oneshot回复，使用Rust名称，空项为None。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib        mcl/core/src/lib.rs；mcl/plugin/src/lib.rs  解析入口与队列扩展；Plugin安装
# components mcl/core/src/components.rs                MclQueue
# events     mcl/core/src/events.rs                    MclCommand
# system     mcl/plugin/src/system.rs                  请求、闭包结果与队首路由
# handler    mcl/plugin/src/handler.rs                 参数求值及领域调用
# types      mcl/plugin/src/types.rs                   执行上下文和异步载荷
# types pub  mcl/core/src/types.rs                     指令、语法树、选择器与结果值
# error      mcl/core/src/error.rs                     MclError
```

# lib

## 函数

公开，定义于mcl_core：
```text
parse_instruction(instruction: &MclInstruction) -> Result<MclOperation, MclError>
    行为：按IMPORT、CREATE、GET、INJECT、CONFIG、EFFECT分流；检查语法和问号参数数量，不访问World或文件。
```

## 类型

公开，定义于mcl_core：
```text
WorldMclExt：队列访问扩展，公开trait
    mcl_close(&mut self, target: Entity) -> Result<(), MclError>
        行为：关闭入队门，取消在途命令，排队项转入取消收口。
    mcl_drain(&self, target: Entity) -> Result<MclDrain, MclError>
        行为：取得共享收口等待能力。
    impl WorldMclExt for World
```

公开，定义于mcl_plugin：
```text
MclPlugin：MCL执行插件
    schedule: String
    impl Plugin for MclPlugin
        build(self, app: &mut App)
            行为：检查运行设施，安装mcl_command_system，不提供Lua宿主函数。
```

# components

## 类型

公开：
```text
MclQueue：目标Entity的命令队列，公开组件
    commands: VecDeque<MclCommand>
    pending: Option<String>--当前队首id
    accepting: bool
    drain: MclDrain
    impl Component for MclQueue
```

# events

## 类型

公开：
```text
MclCommand：命令请求，公开事件
    id: String
    source: Entity--由网络或Driver宿主绑定的真实来源
    source_address: String--调用入口必需显式提供自身真实地址；校验与source一致，历史source不记录Entity编号
    source_instance: Option<String>--Driver来源为本次start_id，客户端为None；不引用Lua core类型
    target: Entity
    instruction: MclInstruction
    cancel: CancellationToken
    reply: Arc<Mutex<Option<oneshot::Sender<Result<MclValue, MclError>>>>>
    impl Event for MclCommand
```

# system

## 函数

crate公开：
```text
mcl_command_system(world: &mut World)
    处理事件：MclCommand
    接收闭包结果：Result<CommandOutput, CommandError>
    提交闭包：execute_command(CommandContext) -> Future<Output = Result<CommandOutput, CommandError>>
    产出事件：None
    oneshot回复：Result<MclValue, MclError>
    行为：MclCommand -> handle_command；Result<CommandOutput, CommandError> -> handle_command_result；MclQueue -> handle_queue。
```

# handler

## 函数

crate公开：
```text
handle_command(world: &mut World, request: MclCommand)
    行为：核对来源和目标完整Entity、Driver来源实例、入队门及重复id；入目标FIFO；拒绝通过reply返回错误。

handle_queue(world: &mut World, target: Entity)
    行为：队首无pending时解析一次，取得owned执行能力，设置pending并提交execute_command；关闭的排队项完成取消收口，不提交业务动作。

handle_command_result(world: &mut World, result: Result<CommandOutput, CommandError>)
    行为：核对目标、pending和队首id，回复原oneshot并移除队首；异步任务及回执全部收口后通知drain。
```

私有：
```text
execute_command(context: CommandContext) -> Future<Output = Result<CommandOutput, CommandError>>
    行为：按语法树执行一次操作，等待存储或领域oneshot结果；不跨await借用World，不重复解析或审计。

execute_import(context: CommandContext, kind: ImportKind, locator: String, alias: String)
    行为：校验声明类型和Driver依赖；通过address定位文件；在所属领域路径集合登记alias，不保存来源ID或正文，不自动注入、开放工具或激活沙箱。

execute_get(context: CommandContext, expression: MessageExpression)
    行为：按顺序拼接消息来源；选择器统一读事务；Prompt alias按路径读取并保留消息角色；返回.Array.Message。

execute_inject(context: CommandContext, source: MessageParameter, target: Selector)
    行为：准备消息来源，在一次数据库写事务中读取目标、校验范围并替换；自引用先读后写；失败不修改字段。

execute_config(context: CommandContext, operation: ConfigOperation)
    行为：只访问当前Agent设置文件；GET读取整份TOML；SET按点分路径赋值；DELETE删除键；写入完整成功才返回。

execute_effect(context: CommandContext, name: String, arguments: Vec<EffectParameter>)
    行为：按effect参数约束求值，通过对应领域core能力执行或发送带oneshot的具体领域请求；保留本次请求与回合关联。
```

# types

## 类型

私有：
```text
CommandContext：执行材料--原MclCommand、MclOperation、owned存储与执行能力、路径和取消能力
CommandOutput：完成载荷--原请求关联、MclValue
CommandError：失败载荷--原请求关联、MclError
```

# types pub

## 类型

公开：
```text
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
    Expression(MessageExpression)
    Manual(usize)

MclValue：命令成功值，公开枚举
    Unit
    Array { Message: Vec<serde_json::Value> }
    Config(toml::Table)
    Inference { message: serde_json::Value, stop_reason: String }
    Import { kind: ImportKind, alias: String }

MclDrain：线程安全收口能力
    wait(&self) -> Future<Output = Result<(), MclError>>
        行为：等待队列、在途操作及回执全部完成，未解决清理失败返回错误。
```

# error

## 类型

公开：
```text
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

Lua Runtime中的MclHost
├── 真实source与VM实例关联
└── MclCommand.reply接收端

mecs全局异步设施
└── execute_command
    ├── owned执行能力
    └── 存储及领域请求的oneshot接收端
```
