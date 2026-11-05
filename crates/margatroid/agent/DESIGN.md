本文描述本轮源码实现的中止与ToolTask生命周期契约；不表示用户已部署或正在运行的daemon已更新。

# 伪代码格式

```text
板块：一级标题，一行注明core与plugin文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列出字段；函数列出签名及直接行为。
System分别列出处理事件、接收闭包结果、提交闭包与产出事件，使用Rust类型和函数名；空项使用None。
属性不写Rust Attribute；持有关系使用树形图。
```

# 板块格式

```text
# lib        agent/core/src/lib.rs；agent/plugin/src/lib.rs  Agent自身扩展；Plugin安装
# components agent/core/src/components.rs                    Agent组件
# events     agent/core/src/events.rs                        启停、重启及异常事件
# system     agent/plugin/src/system.rs                      事件收集与路由
# handler    agent/plugin/src/handler.rs                     同步受理、异步流程与结果收口
# types      agent/plugin/src/types.rs                       流程参数与闭包输出
# types pub  agent/core/src/types.rs                         AgentState与启动参数
# error      agent/core/src/error.rs                         AgentError
```

# lib

## 类型

公开，定义于agent_core：
```text
WorldAgentExt：Agent访问扩展，公开trait
    agent_state(&self, agent: Entity) -> Result<&AgentState, AgentError>
        行为：读取Agent状态，目标不存在返回错误。
    agent_start_turn(&mut self, agent: Entity) -> Result<String, AgentError>
        行为：仅Idle允许；同步生成turn并转Working。
    agent_finish_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>
        行为：仅Working且本次推理调用链已收口时转Idle，不等待已return未finish的后台工具任务；未return工具的中止结果先交付，不以其自然结束作为Driver收尾条件；并清空turn；有活动turn时核对匹配，中止后turn为空由当前Driver调用链完成；调用方plugin核对VM实例，拒绝旧实例完成。
    agent_abort_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>
        行为：仅Working且turn匹配时同步清空turn，保持Working；否则返回错误。此操作必须先于同帧MCL、推理和工具调用受理；不在agent_core跨域取消任务，调用方随后通过各领域core执行推理取消和ToolTasks未return任务的原子强制return及清理。
    impl WorldAgentExt for World
        行为：只访问Agent自身组件，不操作其他领域。
```

公开，定义于agent_plugin：
```text
AgentPlugin：生命周期插件，公开类型
    schedule: String--启停System所属计划
    impl Default for AgentPlugin
        行为：使用RuntimePlugin::UPDATE。
    impl Plugin for AgentPlugin
        build(self, app: &mut App)
            行为：
                检查运行设施和计划，拒绝重复安装。
                在同一schedule依次add_system：stop_agent_system、restart_agent_system、start_agent_system。
                安装异常System；各启停System收集自己的闭包结果。
                使用mecs按挂载顺序同步执行的机制，不增加生命周期排序器或统一请求枚举。
```

# components

## 类型

公开：
```text
Agent：Agent领域组件，公开结构体
    name: String
    workspace: Entity
    state: AgentState
    turn: Option<String>
    impl Component for Agent

```

# events

## 类型

公开：
```text
StartAgent：启动请求，公开事件
    id: String
    workspace: Entity
    name: String
    parameters: AgentStartParameters
    reply: Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>
    impl Event for StartAgent

StopAgent：关闭请求，公开事件
    id: String
    agent: Entity
    reply: Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>
    impl Event for StopAgent

RestartAgent：重启请求，公开事件
    id: String
    workspace: Entity
    name: String
    parameters: AgentStartParameters
    reply: Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>
    impl Event for RestartAgent

AgentException：异常输入，公开事件
    id: String
    agent: Entity
    error: AgentError
    impl Event for AgentException
```

# system

## 函数

crate公开：
```text
stop_agent_system(world: &mut World)
    处理事件：StopAgent
    接收闭包结果：Result<StopOutput, StopError>
    提交闭包：stop_flow(FlowContext) -> Future<Output = Result<StopOutput, StopError>>
    产出事件：None
    oneshot回复：Result<Entity, AgentError>
    行为：StopAgent -> handle_stop_agent；Result<StopOutput, StopError> -> handle_stop_result。

restart_agent_system(world: &mut World)
    处理事件：RestartAgent
    接收闭包结果：Result<RestartOutput, RestartError>
    提交闭包：restart_flow(FlowContext) -> Future<Output = Result<RestartOutput, RestartError>>
    闭包产出事件：InitToolTasks、InitInference、StartDriverLuaVm、StopDriverLuaVm
    产出事件：None
    oneshot回复：Result<Entity, AgentError>
    行为：RestartAgent -> handle_restart_agent；Result<RestartOutput, RestartError> -> handle_restart_result。

start_agent_system(world: &mut World)
    处理事件：StartAgent
    接收闭包结果：Result<StartOutput, StartError>
    提交闭包：start_flow(FlowContext) -> Future<Output = Result<StartOutput, StartError>>
    闭包产出事件：InitToolTasks、InitInference、StartDriverLuaVm
    产出事件：None
    oneshot回复：Result<Entity, AgentError>
    行为：StartAgent -> handle_start_agent；Result<StartOutput, StartError> -> handle_start_result。

agent_exception_system(world: &mut World)
    处理事件：AgentException
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    行为：AgentException -> handle_agent_exception。
```

# handler

## 函数

crate公开：
```text
handle_start_agent(world: &mut World, request: StartAgent)
    行为：校验Workspace和成员创建请求；同步创建Agent Entity并挂Agent等基础组件，置Starting，再提交start_flow；拒绝时发送启动错误。

handle_stop_agent(world: &mut World, request: StopAgent)
    行为：按白名单同步转Stopping；关闭新请求受理，再提交stop_flow；拒绝时发送关闭错误。

handle_restart_agent(world: &mut World, request: RestartAgent)
    行为：在request.workspace内按name定位成员Entity，先检查ToolTasks，存在尚未finish的活任务条目直接拒绝；通过白名单后同步转Restarting，关闭新请求受理；使用request.parameters提交restart_flow；失败通过原reply回复。

handle_agent_exception(world: &mut World, event: AgentException)
    行为：核对Entity，关闭新受理并报告event.error，不发送额外异常结果事件。

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
handle_restart_result(world: &mut World, result: Result<RestartOutput, RestartError>)
    行为：
        核对Entity及对应生命周期状态；忽略失效结果。
        启动成功：转Idle，通过原StartAgent.reply回复Entity。
        重启成功：转Idle，通过原RestartAgent.reply回复Entity。
        关闭成功：销毁Entity，通过原StopAgent.reply回复原Entity。
        失败：有完整清理证明则销毁Entity，否则保留宿主及当前生命周期状态；启停和重启失败均通过原reply回复错误。

```

私有：
```text
start_flow(context: FlowContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：
        调用address::resolve_file及agent_driver_core::read_driver取得本次Driver材料，准备数据库与设置。
        运行组件初始化阶段先发送InitToolTasks和InitInference，各携带agent及独立oneshot发送端，不先等待其中一个再发另一个。
        使用tokio::join!等待两个接收端，与原有资源初始化一起完成；收齐全部结果后处理错误，不提前丢弃另一项。
        全部初始化成功后准备agent_info并发送StartDriverLuaVm，等待原reply；mcl和wait由Lua Runtime构造。
        失败清理本次已创建内容并返回，成功返回；core不引用其他core的组件类型。

stop_flow(context: FlowContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：请求Driver停止和执行取消；等待各任务自身的退出结果及MCL收口；返回清理结果，不销毁协调Entity。

restart_flow(context: FlowContext) -> Future<Output = Result<RestartOutput, RestartError>>
    行为：await stop_flow，成功后使用重启请求提供的parameters执行start_flow；全程Restarting，不发公开StopAgent或StartAgent事件。
```

# types

## 类型

私有：
```text
FlowContext：闭包上下文
    id: String--原请求ID
    agent: Entity
    events: RuntimeEventSender
    parameters: Option<AgentStartParameters>--启动与重启为Some，关闭为None
    reply: Option<oneshot::Sender<Result<Entity, AgentError>>>--原请求回复端

StartOutput / StopOutput / RestartOutput：对应闭包的成功载荷
    id: String--原请求ID
    agent: Entity

StartError / StopError / RestartError：对应闭包的失败载荷
    id: String--原请求ID
    agent: Entity
    error: AgentError
    cleanup_completed: bool

```

# types pub

## 类型

公开：
```text
AgentStartParameters：启动参数，公开类型
    driver: String--Driver文件定位字符串
    project_root: PathBuf
    home_root: PathBuf
    database_path: PathBuf
    settings_path: PathBuf

AgentState：生命周期状态，公开枚举
    Starting--仅允许匹配当前启动操作的步骤与结果
    Idle--允许开始回合、StopAgent、RestartAgent
    Working--允许结束或中止当前回合、StopAgent、RestartAgent
    Restarting--仅允许匹配当前操作的步骤与结果
    Stopping--仅允许匹配当前操作的步骤与结果

```

# error

## 类型

公开：
```text
AgentError：生命周期错误，公开类型
    kind: AgentErrorKind
    message: String
    impl Display + std::error::Error

AgentErrorKind：错误类别，公开枚举
    InvalidRequest | AgentMissing | StateRejected
    StartFailed | StopFailed | RestartFailed | CleanupFailed | ReplyClosed
```

# 持有关系

```text
World
└── Agent Entity
    ├── Address（address）
    ├── Agent（agent_core）
    ├── ToolTasks（tool_core，InitToolTasks挂载）
    ├── ActiveInference（inference_core，InitInference挂载）
    ├── Driver运行组件（Lua领域core）
    ├── MclQueue（MCL领域core）
    ├── 数据库与历史组件（存储领域core）
    └── 各资源路径集合（所属领域core）

异步闭包
└── FlowContext
    ├── Entity
    ├── owned参数及事件发送能力
    └── 本步骤oneshot接收端
```
